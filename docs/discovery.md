# Discovery and optional hooks

Native KelpMux panes (served by [KelpServer](https://github.com/joanfabregat/kelpcore)) consume [OSC 7501 program status](https://github.com/joanfabregat/kelpcore/blob/main/docs/agent-polling.md#terminal-program-status) from Codex and Claude Code. The PTY identifies the pane directly, so activity no longer requires a Codex transport bridge or a thread-to-terminal guess. Process discovery and read-only daemon polling still supply supplementary metadata when a conversation ID is known. The passive identity limitations below still apply; activity and conversation identity are separate observations.

## Default: discover existing processes

Start agents directly in tmux as usual. `kelpmux daemon` discovers them without a launch wrapper. It follows the pane shell's Linux process tree and reads metadata when an agent exposes a reliable identity. `kelpmux scan` performs one pass. Nothing pings an agent with simulated user input or starts another agent process.

| Agent | Identity | Name | Status |
| --- | --- | --- | --- |
| Claude Code | `~/.claude/sessions/PID.json`, validated against PID and Linux birth ticks; explicit resume ID as fallback | Live record name, then directory | Live status record when supported |
| Codex | Primary process `CODEX_THREAD_ID`, explicit resume ID, or a rollout descriptor open in that process | Session index and existing daemon thread metadata | Existing daemon `thread/read` status, or bounded structured rollout lifecycle events |
| Either, unsupported metadata | Agent process is still detected; session ID is null | Directory or user override | UNKNOWN |

Use `kelpmux name --pane %ID 'Display name'` for an override. Session-bound overrides survive agent restarts if the ID is discovered again. Process-bound overrides last for that process identity.

The adapters were developed against Codex 0.162.1 and Claude Code 2.1.296 metadata. These interfaces can change. The beta cannot identify every Codex shared-daemon TUI: a shared server holding several rollout descriptors does not prove which TUI owns which conversation. Starting with `codex resume SESSION_ID` exposes an identity without using a Kelpmux launcher, although switching to a different conversation inside that TUI can invalidate the original argument. A fresh/shared-daemon TUI may need optional hooks with reliable pane correlation to supply its ID. `resume --last` and interactive session pickers do not expose a reliable ID by themselves.

### Shared-daemon identity investigation (Codex 0.166.0)

The installed version was inspected against the pinned upstream source. Its [TUI status emitter](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/tui/src/terminal_program_status.rs) sends OSC 7501 state, application and blocking kind, without a thread ID. The [loaded-thread response](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/src/protocol/v2/thread.rs) returns thread IDs and pagination, without a terminal/client PID association. The [initialization client information](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/app-server-protocol/src/protocol/v1.rs) identifies the client application and version rather than the process. [SessionStart hook requests](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/hooks/src/events/session_start.rs) supply a session ID and directory, but no displaying TUI PID or terminal. Shared daemon logs also cannot be treated as a supported active-terminal ownership API.

On the inspected preprod pane, the TUI was launched as plain `codex`, lacked `CODEX_THREAD_ID`, and held no rollout descriptor; its conversation file was held by a shared app-server worker. This explains the unknown identity independently of its deleted working directory. Folder/title matching cannot establish ownership when names repeat, threads switch, or several clients share a project. The fallback title parser handles the Linux ` (deleted)` suffix, but does not create an identity association.

Recommended upstream integration: emit the active conversation ID directly from the TUI as terminal metadata, updating it on resume/new/switch and clearing it on exit or handoff. Kelpmux can bind that event to the emitting PTY and process lifetime, then use its existing read-only daemon polling for the name and supplementary status. Until a verified signal is available, keep unresolved identities unknown and retain the explicit conversation picker. Hooks require proven per-client correlation; adding a hook to a shared daemon with inherited pane variables alone does not establish that correlation.

Names and session IDs are separate fields. UNKNOWN means that evidence is unavailable; agent idle and turn finished are only shown when the adapter or hooks supply that lifecycle information. A process exiting produces an EXITED journal transition and clears the pane's agent identity. Its fixed palette color remains; an ordinary shell shows ○ idle and a running non-agent command shows ● working. A retained dead pane shows ■ exited. Discovery cannot distinguish a crash from an intentional exit solely from a vanished PID.

## Optional: lifecycle hooks

Hooks supply exact lifecycle transitions when supported by the agent. They supplement discovery; they do not require launching agents through Kelpmux. The receiver reads JSON from stdin, identifies a currently discovered agent in a pane, stores only session identity/status, emits no stdout, and returns. The daemon then refreshes headers on its next scan. Commands end in `|| true` so receiver failures do not decide tool approvals or block the agent. A three-second timeout bounds hook latency.

Before enabling hooks, check that their process inherits the correct `TMUX_PANE` and tmux socket context. Hooks in a shared Codex daemon can inherit the pane where that daemon originally started, rather than the current TUI's pane. **Do not enable the generic example for multiple such clients unless pane correlation is verified.** Kelpmux rejects a hook whose session ID conflicts with an already identified pane, but an unidentified pane cannot detect a wrongly inherited identity. Hooks are not a universal solution to missing shared-daemon ownership metadata.

If your integration supplies a verified pane ID, use `kelpmux event codex --pane %ID` (or `claude`) rather than relying on the environment. For a custom tmux server/state directory, include the same `--tmux-socket PATH --state-dir PATH` as the daemon. A hook can run a one-shot discovery scan, so the daemon need not already be running to journal it. Do not derive pane IDs by directory matching when several agents share a project.

### Claude Code

Review [examples/claude-hooks.json](../examples/claude-hooks.json) and merge its `hooks` entries into your existing Claude settings. Preserve all existing entries; this is a fragment, not a replacement settings file. Set an absolute binary path if the hook PATH does not contain Kelpmux. Hooks take effect according to Claude's settings lifecycle.

The example covers session start/end, prompt submit, tool use, permission requests, notifications, successful stops, and failed stops. `AskUserQuestion` is input needed. Permission/idle notifications are input needed; unrelated notifications are ignored. A Stop with `stop_hook_active=true` is ignored so an already active stop hook does not falsely mark completion.

### Codex

Review [examples/codex-hooks.toml](../examples/codex-hooks.toml) and merge the event tables into `config.toml`; preserve existing handlers. Codex currently supports inline TOML command hooks. Use `/hooks` to review/trust new definitions. Check your installed version's supported events before enabling them. This example covers session start/end, prompt submit, tool use, permission requests, and Stop. `request_user_input` is input needed when exposed through PreToolUse. Features or orchestration modes can omit hooks; missing events must not be treated as reliable state.

Both examples use the same receiver format: `session_id`, `hook_event_name`, and, where relevant, `tool_name`, `notification_type`, and `stop_hook_active`. Prompt text, tool arguments, transcript paths, and response content are discarded. The legacy Codex `agent-turn-complete` payload with `thread-id` is also recognized, but it only signals completion and does not replace lifecycle hooks.

A hook takes precedence for five seconds to bridge metadata-update delays. After that, a supported live status takes precedence; unknown live status continues to use the hook evidence. An observed working-to-idle transition is retained as TURN FINISHED until another turn starts. If events stop arriving and native status is unknown, hook-derived status can be stale; consult `last_seen`, `source`, and the actual TUI.

Sources: [Claude Code hook reference](https://code.claude.com/docs/en/hooks), [Codex hook reference](https://learn.chatgpt.com/docs/hooks), and [Codex app-server protocol](https://github.com/openai/codex/tree/main/codex-rs/app-server). Agent upgrades should rerun the fixtures and verify identity against the actual TUI before relying on recovery records.

## Owned native workspaces

Native workspaces are served by [KelpServer](https://github.com/joanfabregat/kelpcore), which performs discovery itself; no separate tmux daemon or launch wrapper is needed. Its native hook receiver (`kelpmux native-event codex|claude`), pane identity rules, tab labels, recovery metadata and the shared polling adapters are documented in [KelpServer's discovery guide](https://github.com/joanfabregat/kelpcore/blob/main/docs/discovery.md) and [agent polling adapters](https://github.com/joanfabregat/kelpcore/blob/main/docs/agent-polling.md). The sections above cover the legacy tmux path, whose commands (`kelpmux daemon`, `scan`, `name` and `event`) remain part of KelpMux.
