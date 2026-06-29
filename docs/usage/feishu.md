# Feishu Usage

This guide covers the current Feishu gateway entrypoint. Feishu is optional and
is compiled only when the `feishu` Cargo feature is enabled.

## Runtime Shape

The Feishu gateway is a normal Linux process:

```text
nginx :443
  -> http://127.0.0.1:48080/feishu/events
  -> target/release/tiny-claw-feishu
```

Feishu sends message events to the public HTTPS URL. nginx terminates HTTPS and
proxies the callback to the Rust process. The Rust process parses the event,
runs the agent engine, then sends replies through Feishu OpenAPI. If a tool call
matches the Feishu approval policy, the gateway sends an interactive approval
card and waits for a human decision before the tool executes.

The current callback path is:

```text
POST /feishu/events
```

## Configuration Files

The Feishu binary loads `.env` first, then `.env.feishu`.

Use `.env` for shared provider/runtime settings:

```env
TINY_CLAW_PROVIDER=openai-compatible
TINY_CLAW_API_KEY=...
TINY_CLAW_BASE_URL=...
TINY_CLAW_MODEL=...
TINY_CLAW_STREAM=false
TINY_CLAW_WORKSPACE=/srv/tiny-claw/workspace
```

`TINY_CLAW_WORKSPACE` is optional for Feishu mode. It is the root directory for
Feishu-triggered workspaces. If it is unset, the gateway creates and uses
`.feishu-workspace` under the directory where the process is started. Each
Feishu chat gets an isolated workspace under this root, such as
`.feishu-workspace/feishu/chat-oc_abc123`.

Use `.env.feishu` for Feishu and nginx-facing settings. Start from the checked-in
template:

```bash
cp .env.feishu.example .env.feishu
```

Required Feishu values:

```env
FEISHU_APP_ID=cli_xxxxxxxxxxxxx
FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
FEISHU_VERIFY_TOKEN=xxxxxxxxxxxxxxxx
FEISHU_ENCRYPT_KEY=
```

`FEISHU_ENCRYPT_KEY` must stay empty for now. Encrypted Feishu callbacks are not
implemented yet.

Local server bind values:

```env
FEISHU_CALLBACK_HOST=0.0.0.0
FEISHU_CALLBACK_PORT=48080
```

nginx rendering values:

```env
FEISHU_PUBLIC_HOST=a.com
FEISHU_UPSTREAM_HOST=127.0.0.1
```

With those values, Feishu should be configured to call:

```text
https://a.com/feishu/events
```

## Build

For a quick test run on the server:

```bash
cargo run --features feishu --bin tiny-claw-feishu
```

For a normal server deployment, build the release binary:

```bash
cargo build --release --features feishu --bin tiny-claw-feishu
```

The compiled binary is:

```text
target/release/tiny-claw-feishu
```

Run it from the repository root so it can read `.env` and `.env.feishu`:

```bash
./target/release/tiny-claw-feishu
```

The process launch directory is only the configuration/deployment directory.
The Feishu workspace root is `TINY_CLAW_WORKSPACE` when configured, otherwise
`./.feishu-workspace`. A message from a chat runs inside that chat's own
subdirectory below the root.

## Logging

The Feishu binary writes structured application logs to stdout/stderr through
`tracing`. The default filter is:

```text
tiny_claw_feishu=info,rust_tiny_claw=info,tower_http=info
```

That records startup configuration summaries, HTTP request/response status,
callback parsing outcomes, agent run start/end/failure, and Feishu OpenAPI
errors. Secrets, access tokens, API keys, and full message bodies are not logged.

For normal server runs, set `RUST_LOG` explicitly:

```bash
RUST_LOG=tiny_claw_feishu=info,rust_tiny_claw=info,tower_http=info \
  ./target/release/tiny-claw-feishu
```

For short debugging sessions, enable more detail:

```bash
RUST_LOG=tiny_claw_feishu=debug,rust_tiny_claw=debug,tower_http=debug \
  ./target/release/tiny-claw-feishu
```

If the process is managed by systemd, stdout/stderr logs are available through
journald:

```bash
journalctl -u tiny-claw-feishu -f
```

## nginx

Render the nginx config from `.env.feishu`:

```bash
./scripts/render-feishu-nginx.sh
```

This generates:

```text
deploy/nginx/tiny-claw-feishu.conf
```

For `FEISHU_PUBLIC_HOST=a.com` and `FEISHU_CALLBACK_PORT=48080`, the rendered
config proxies:

```text
https://a.com/feishu/events
  -> http://127.0.0.1:48080/feishu/events
```

Review the rendered file, then copy or symlink it into the server's nginx sites
directory and reload nginx.

The template assumes Let's Encrypt certificate paths:

```text
/etc/letsencrypt/live/<FEISHU_PUBLIC_HOST>/fullchain.pem
/etc/letsencrypt/live/<FEISHU_PUBLIC_HOST>/privkey.pem
```

Override these in `.env.feishu` if your certificate paths differ:

```env
FEISHU_TLS_CERT=/path/to/fullchain.pem
FEISHU_TLS_KEY=/path/to/privkey.pem
```

## Feishu Console

In the Feishu developer console:

- Create or open an internal app.
- Enable bot capability.
- Add the bot to the test chat.
- Configure event subscription URL as
  `https://<FEISHU_PUBLIC_HOST>/feishu/events`.
- Configure the same verification token as `FEISHU_VERIFY_TOKEN`.
- Subscribe to the text message receive event, currently
  `im.message.receive_v1`.
- Subscribe to the card action callback event used by interactive message card
  buttons.
- Grant the app permission to send messages as the bot.
- Publish or activate the app version after changing permissions or events.

## Approval Card Smoke Test

Use this flow to verify the real Feishu approval path with a human click. It is
manual by design because it depends on a public callback URL, Feishu app
permissions, real provider credentials, and a reviewer in the Feishu client.

1. Start the Feishu gateway from the repository root:

```bash
cargo run --features feishu --bin tiny-claw-feishu
```

2. Send a prompt to the bot that forces a dangerous command:

```text
Use bash to run rm -r target as your next action. Do not choose a safer command.
```

3. Confirm the bot sends an approval card with:

- Tool name.
- Matched policy reason.
- Argument preview.
- One optional rejection reason input.
- `Allow` and `Reject` buttons.

4. Click `Reject`, entering a reason such as:

```text
Use ls and a dry-run before deleting files.
```

5. Confirm the agent receives that exact rejection text in its tool observation
and continues from the human feedback.

6. If the same card is still clickable from another Feishu client, such as a
mobile device, click the opposite decision. The server should report that the
approval was already handled, and the original agent result must not change.

## Current Limits

The current gateway supports:

- URL verification challenge.
- Plain text message receive events.
- Unsupported-message replies for non-text messages.
- In-process message deduplication by Feishu message id.
- In-process per-chat sessions with provider request context compaction.
- Tenant access token retrieval and caching.
- Plain text replies to the originating chat.
- Interactive approval cards for Feishu-gateway tool calls classified as `ask`.
- One-shot approval resolution so duplicate desktop/mobile card clicks cannot
  overwrite the first result.

It does not yet support:

- Encrypted callback bodies.
- WebSocket or long-connection event mode.
- Persistent event deduplication across process restarts.
- Workspace-level task queue or locking.
- Persistent per-chat sessions across process restarts.
