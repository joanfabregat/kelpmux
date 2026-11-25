# KelpMux configuration

KelpMux keeps a user's settings in one TOML file, `~/.config/kelpmux/config.toml` (`$XDG_CONFIG_HOME/kelpmux/config.toml` when `XDG_CONFIG_HOME` is set). It is the single place for user configuration; later releases will also keep the list of projects there. Run `kelpmux config path` to print its path: it creates the file first if there is none, with every key commented out at its default and documented.

Every key is optional. Folder paths are absolute or start with `~/` (your home folder; `~` alone is the home folder itself); other `~` forms, relative paths and `..` are refused. Unknown keys are errors, with a "did you mean" suggestion when one is close, so a typo never goes unnoticed.

## Editing

- **By hand or by an agent**: edit the file, then run `kelpmux config check`.
- **From the web client**: Settings, **Files & folders**. The bridge validates a change with the same rules as `kelpmux config check` and saves it only when the whole file is valid. It edits the file in place, keeping its comments and formatting, and a key that was commented out at its default is written where its documentation is.

Several web connections may save at the same time: each save takes a `flock` on `.config.toml.lock` beside the file, writes a temporary file in the same folder, fsyncs it and renames it over the file. When `config.toml` is a symbolic link (from a dotfiles manager, say), saves replace the file it points to and the link stays.

## Live reload

Every web bridge (one per browser connection) watches the file with inotify, plus a two-second check of its metadata that also notices a linked file changing. When the file changes, the bridge validates it. A valid file applies at once and the browser shows the new values. An invalid file is not applied: the last valid configuration stays in effect, and Settings shows every problem as `line:column: message` in a banner at the top of the dialog, with a warning mark on **Files & folders** and on the Settings button until the file is fixed.

Some keys only take effect when the workspace server starts. `files.root` and `files.extra_roots` are KelpServer's file roots: `kelpmux start` (and the web client's **Start workspace**, which runs it) records them in the workspace's `workspace.json` as `files_root` and `files_extra_roots` when it launches a server, and the server reads them when it starts. A running workspace keeps its roots until it is stopped and started again; Settings says when the saved root differs from the one the running server uses. Without a configuration file `workspace.json` is left as it is, and with an invalid one `kelpmux start` prints the problems and leaves the roots as they were rather than refusing to start.

## Changing KelpMux config (for agents)

1. Run `kelpmux config path` and edit that file. Keep its comments; uncomment a key to change it. The reference below lists every key.
2. Run `kelpmux config check`. It prints each problem as `path:line:col: message` and exits with status 1 when the file is invalid, 0 when it is valid. Fix every problem it reports.
3. Changes apply live in every open web client; nothing needs restarting, except that a changed `files.root` or `files.extra_roots` applies on the next workspace start (`kelpmux stop`, then `kelpmux start`, which ends the workspace's shells: ask the user first).

`kelpmux config schema` prints a JSON Schema of the file for editors and tools; `kelpmux config check` also enforces the rules a schema cannot express (folders that must exist, folders that must lie inside the roots).

## Commands

| Command | Does |
| --- | --- |
| `kelpmux config path` | Prints the file's path, creating the documented default file if there is none. |
| `kelpmux config check [FILE]` | Validates the file (default: the user's). Prints problems as `path:line:col: message`; exit status 0 when valid, 1 when not. A missing file is valid: the defaults apply. |
| `kelpmux config schema` | Prints the JSON Schema (draft 2020-12). |
| `kelpmux config docs` | Prints the key reference below. |

# Reference

The reference is generated from the key definitions in `src/native/user_config.rs` by `kelpmux config docs`; a unit test fails when it differs from this file.

<!-- BEGIN kelpmux config docs -->
## Top level

### `version`

Schema version of this file. KelpMux reads version 1 and refuses a file written for a newer version rather than misreading it.

| | |
| --- | --- |
| Type | integer |
| Default | `1` |
| Applies | Immediately. |

## `[files]`

The web client's file browser.

### `files.root`

The folder the web file browser is confined to: KelpServer refuses every file operation, download and upload destination outside it and outside `files.extra_roots`. It must be an existing folder. This limits the file browser, not you: a shell in a terminal can still reach anything your account can.

| | |
| --- | --- |
| Type | folder path (string) |
| Default | `"~"` |
| Applies | On the next workspace start: KelpServer reads its file root when it starts (`kelpmux start` records it in the workspace's `workspace.json` as `files_root`). |

### `files.extra_roots`

Folders outside `files.root` that the file browser may also open, such as a data drive. Each must be an existing folder that is not inside `files.root`; at most 16, without duplicates.

| | |
| --- | --- |
| Type | list of folder paths (strings) |
| Default | `[]` |
| Example | `extra_roots = ["/mnt/data"]` |
| Applies | On the next workspace start (recorded as `files_extra_roots` in `workspace.json`), with a KelpServer that supports extra roots; an older server ignores them. |

## `[projects]`

Projects.

### `projects.base`

The default folder for new projects: a project named `name` goes in `<base>/<name>`. It must lie inside `files.root` or one of `files.extra_roots`, and need not exist yet.

| | |
| --- | --- |
| Type | folder path (string) |
| Default | `"~/projects"` |
| Applies | Immediately. Stored for project support in a later release; nothing uses it yet. |

## `[scratch]`

Terminals outside any project.

### `scratch.folder`

The folder terminals outside any project start in. It must lie inside `files.root` or one of `files.extra_roots`, and need not exist yet.

| | |
| --- | --- |
| Type | folder path (string) |
| Default | `"~/scratch"` |
| Applies | Immediately. Stored for a later release; nothing uses it yet. |

<!-- END kelpmux config docs -->
