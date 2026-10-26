# Discovery and optional hooks

## Default: discover existing processes

Start agents directly in tmux as usual. `kelpmux daemon` discovers them without a launch wrapper. It follows the pane shell's Linux process tree and reads metadata when an agent exposes a reliable identity. `kelpmux scan` performs one pass. Nothing pings an agent with simulated user input or starts another agent process.

| Agent | Identity | Name | Status |
| --- | --- | --- | --- |
| Claude Code | `~/.claude/sessions/PID.json`, validated against PID and Linux birth ticks; explicit resume ID as fallback | Live record name, then directory | Live status record when supported |
| Codex | Primary process `CODEX_THREAD_ID`, explicit resume ID, or a rollout descriptor open in that process | Session index and existing daemon thread metadata | Existing daemon `thread/read` status, or bounded structured rollout lifecycle events |
| Either, unsupported metadata | Agent process is still detected; session ID is null | Directory or user override | UNKNOWN |

Use `kelpmux name --pane %ID 'Display name'` for an override. Session-bound overrides survive agent restarts if the ID is discovered again. Process-bound overrides last for that process identity.

The adapters were developed against Codex 0.162.1 and Claude Code 2.1.296 metadata. These interfaces can change. The beta cannot identify every Codex shared-daemon TUI: a shared server holding several rollout descriptors does not prove which TUI owns which conversation. Starting with `codex resume SESSION_ID` exposes an identity without using a Kelpmux launcher, although switching to a different conversation inside that TUI can invalidate the original argument. A fresh/shared-daemon TUI may need optional hooks with reliable pane correlation to supply its ID. `resume --last` and interactive session pickers do not expose a reliable ID by themselves.

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

The native server performs discovery itself; no separate tmux daemon or launch wrapper is needed. Its pane children inherit `KELPMUX_PANE` and `KELPMUX_STATE_DIR`. An optional native hook uses `kelpmux native-event codex` or `kelpmux native-event claude` with JSON on stdin; `--pane ID --state-dir /absolute/path` overrides the inherited context. The state directory must match the native server, and the receiver rejects missing, exited, reused-PID, mismatched-kind and conflicting-session registrations. Hooks are never installed automatically. Shared agent daemons still require verified pane ownership: inherited context from a different client is not sufficient evidence.

An unidentified agent remains UNKNOWN with a null session identifier. Multiple open Codex rollout descriptors do not prove the current conversation; discovery uses a unique open rollout or an already identified session, never an arbitrary descriptor or another shared server's newest transcript.

Native tab labels prefer an explicit template alias or discovered conversation name over application terminal titles. When metadata has only a directory, a meaningful application OSC title may supply the label instead. Otherwise the label shows the kind and a known ID prefix, or `session unknown`. Folder names remain directory context in the journal, rather than being presented as conversation names in tabs. An application title is a display hint, not proof of session identity.

The metadata recovery helper reads the native journal under a nonblocking lock, prunes records older than seven days and lists the last observed pane, process birth ticks, session ID, display name, directory, status, identity source and timestamps. It also reconstructs sessions from completed journal records if a registry entry is missing. Running records describe the last observation, not a guarantee that the process survived a host crash. Recovery commands are offered only for validated session IDs (`codex resume 'ID'` or `claude --resume 'ID'`); names and directories are displayed as metadata and never interpolated into those commands. Kelpmux does not launch recovery commands or store prompt/tool content.

## Shared polling adapters and explicit links

The native workspace now uses the shared [agent polling adapter registry](agent-polling.md). Codex and Claude use the same scheduler, process discovery and UI status path, with separate metadata adapters. For an unidentified Codex TUI, prefix then `i` opens a read-only picker of loaded daemon conversations. Workspace overview also offers `l` to link its selected tab. Clear a link with the first picker row. Process exit invalidates the link; switching conversations inside a still-running TUI may require relinking. No agent is started or resumed by monitoring.
