# Request User Help Tool

## Purpose

`request_user_help` gives the model an explicit way to stop unproductive tool
retry loops and ask the user for missing information, confirmation, or action.
It is not an approval mechanism and does not grant broader execution
permissions.

## Tool Shape

The tool is registered in `ToolRegistry` with the rest of the workspace tools.
It is read-only because it only reports a structured help request back to the
conversation.

Required arguments:

- `reason`: why the model cannot continue with the current prompt, context, and
  tools.
- `tried`: what the model already attempted or checked.
- `needed`: what information, confirmation, or user action is needed.
- `question`: the concrete question for the user.

The result begins with `USER_HELP_REQUESTED` so reporters, logs, and later
integrations can detect the event without parsing natural language.

## Behavior

The tool validates that each required argument is a non-empty string. A valid
call returns a non-error observation containing the four fields. Invalid calls
return a normal tool error so the model can correct malformed arguments.

The tool description instructs the model to use it only when available tools
cannot produce the missing information or confirmation. After calling it, the
model should stop blind retries and surface the question to the user.

## Boundaries

This is distinct from Feishu approval. Approval means the model knows the action
it wants to perform and needs permission. `request_user_help` means the model is
blocked because it lacks information, context, or a user decision.

The first implementation does not pause the engine or add interactive CLI input.
The help request is returned as an observation and can be included in the final
assistant response.
