# Background Task Notes

## Context

The current `bash` tool is intentionally small: it runs one command in the
workspace, waits for completion, returns stdout/stderr, enforces a timeout, and
lets the model self-correct from command output.

That shape works well for commands such as:

```bash
cargo test
cat Cargo.toml
rg TODO
```

It does not work well for long-running development processes such as:

```bash
npm run dev
python server.py
cargo run
```

Those commands may need to stay alive while later turns call `curl`, inspect a
page, or read logs. If they are executed through the current foreground `bash`
tool, the engine either blocks until the command exits or kills it at the
timeout.

## What `nohup` Means

On Unix-like systems, `nohup` means "no hang up". It starts a process in a way
that ignores the terminal hangup signal, commonly redirects logs to a file, and
then returns control to the shell when combined with `&`:

```bash
nohup npm run dev > server.log 2>&1 &
```

The pieces are:

- `nohup npm run dev`: start the command so it is not stopped by shell hangup.
- `> server.log`: write stdout to a file.
- `2>&1`: write stderr to the same destination.
- `&`: put the process in the background so the shell returns immediately.

The harness should not rely on the model to hand-roll this pattern every time.
Instead, the tool layer should eventually expose an explicit background task
abstraction.

## Preferred Design

Keep `bash` as a foreground command tool and add separate task lifecycle tools:

- `start_bg_task`: start a long-running workspace command and return a task id.
- `read_bg_task`: report whether the task is still running and return recent logs.
- `stop_bg_task`: terminate a task by id.
- `list_bg_tasks`: show active and recently exited tasks.

This keeps foreground command execution and background process management
separate. It also gives the model stable operations instead of asking it to
combine shell redirection, process ids, and log files correctly.

Example flow:

```text
start_bg_task command="npm run dev" name="frontend"
read_bg_task task_id="task_001" lines=80
bash command="curl -i http://localhost:3000"
stop_bg_task task_id="task_001"
```

## Runtime Shape

The first implementation can keep task state in memory:

```rust
struct TaskManager {
    tasks: HashMap<TaskId, ManagedTask>,
}

struct ManagedTask {
    id: TaskId,
    name: Option<String>,
    command: String,
    child: std::process::Child,
    log_path: PathBuf,
    started_at: SystemTime,
}
```

Logs should be written under the existing memory root:

```text
.tiny-claw/tasks/<task_id>.log
```

That gives later turns a stable place to inspect output, while allowing the
process handle to stay inside the runtime.

## Boundaries

The first version should stay local and minimal:

- Only run commands from the workspace.
- Capture stdout and stderr into one log file.
- Return task ids instead of exposing raw process ids as the main interface.
- Provide explicit stop semantics.
- Do not implement remote approvals, Feishu integration, distributed workers, or
  persisted task recovery yet.

Persisting task metadata across engine restarts is useful later, but live child
process handles cannot be restored from disk directly. Recovery should be a
separate design when that becomes necessary.

## Why Not Implement Now

Background task management crosses a different boundary than the foreground
`bash` tool:

- It introduces runtime-owned process lifecycle state.
- It needs log storage and status inspection.
- It may need cleanup on engine shutdown.
- It starts to overlap with memory, middleware, and approval policy.

For now, keep `bash` focused on bounded foreground execution. Add background
task tools later when the project needs long-running local services.
