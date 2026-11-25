# KelpMux configuration

KelpMux keeps a user's settings in one TOML file, `~/.config/kelpmux/config.toml` (`$XDG_CONFIG_HOME/kelpmux/config.toml` when `XDG_CONFIG_HOME` is set). It is the single place for user configuration, the list of projects included (`[[project]]`). Run `kelpmux config path` to print its path: it creates the file first if there is none, with every key commented out at its default and documented.

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

### Projects (for agents)

A project is a `[[project]]` table with an `id`, a `name`, a `folder` and an optional `color` (reference below). To add one, append a table at the end of the file:

```toml
[[project]]
id = "kelp"          # lowercase letters, digits and -, unique, never "scratch"
name = "Kelp"        # shown in the web client; unique regardless of case
folder = "~/projects/Kelp"
color = "teal"       # optional
```

- Choose an `id` no other project uses and keep it when renaming: terminals are linked to their project by it (in the workspace's `terminal-projects.json`), so changing it moves them to Scratch. Do not reuse the id of a deleted project.
- The folder must lie inside `files.root` or `files.extra_roots`, and may not be inside or contain another project's folder or `scratch.folder`. Create it if it does not exist (a project whose folder is missing shows as missing).
- To delete a project, remove its table. Its folder and files stay; its terminals move to Scratch. To delete or archive its folder too, use `kelpmux project` (below).
- Never set `status = "archived"` or write `[project.archive]` by hand: `kelpmux project archive` does, once the archive is verified.
- Scratch is built in: it is not a `[[project]]`, and its folder is `scratch.folder`.
- Run `kelpmux config check` afterwards; open web clients show the change within a few seconds.

`kelpmux config schema` prints a JSON Schema of the file for editors and tools; `kelpmux config check` also enforces the rules a schema cannot express (folders that must exist, folders that must lie inside the roots).

## GitHub

The web client lists the repositories your GitHub account can see (a project's Repos view, **Add repository**), shows read-only facts about the GitHub repositories a project's checkouts come from (visibility, default branch, open pull requests, last push) and clones a repository into a project's folder. It uses the GitHub access the server already has; there is no GitHub App to register.

- **The token** for GitHub's API comes from one source, set in `[github]` (reference below): `auth = "gh"` runs `gh auth token`; `auth = "env"` reads `GH_TOKEN`, then `GITHUB_TOKEN`, from the web bridge's environment; `token_command = "~/bin/github-token"` runs that program, without a shell or arguments, and takes the first line it prints. The web bridge reads it when it needs it and keeps it in memory for five minutes at most. It is sent to `api.github.com` only: never to the browser, a file, a log or Git. A program's error output is discarded, and a failure says only how it ended. The web bridge is started by the web relay's SSH connection, with a login environment: `auth = "env"` needs the variable set there (in `~/.zshenv` or `~/.profile`, say), and `gh` needs `gh auth login` done once on the server. Without a source, or when it fails, Settings, **GitHub** says why and how to fix it; the rest of the web client is unaffected and cloning a URL still works.
- **Scopes**: listing repositories needs `repo` (private repositories) or `public_repo` for a classic token, or Metadata read for a fine-grained one; open pull request counts need Pull requests read on fine-grained tokens. Settings, **GitHub** shows the signed-in login, the token's scopes (classic tokens only: GitHub does not report a fine-grained token's permissions) and the rate limit left, and **Test** reads the token again and checks it.
- **Cloning** runs `git clone` as you, with your own Git configuration and credentials: an SSH agent for `git@github.com:owner/name.git` (the default the picker offers) or your credential helper for `https://github.com/owner/name.git`. KelpMux injects no credential and writes none; the remote stays the URL you chose. Git runs without hooks, templates, fsmonitor, submodules or Git LFS downloads (run `git lfs pull` in the checkout afterwards to fetch LFS content), only over the URL's own transport (HTTPS or SSH, never `file://` or `ext::`), cannot prompt, and is stopped after 15 minutes or when you cancel. It does not get `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN` or `GITHUB_ENTERPRISE_TOKEN` from the web bridge's environment: a `gh auth git-credential` credential helper then uses gh's stored login (`gh auth login`), so with `auth = "env"` alone, HTTPS clones need another credential helper or SSH. The checkout goes in `<project folder>/<name>`, and an existing folder of that name is never replaced. Repositories GitHub reports larger than 5 GiB are refused (clone them in a terminal). A web bridge started over SSH usually has no `SSH_AUTH_SOCK`: set `ssh_auth_sock` to an agent socket that outlives your sessions (one started by a systemd user unit, say) for SSH clones to work.

## Project status, archive, restore and delete

A project's `status` is `active`, `paused` or `archived` (reference below). Pausing only dims the project in the sidebar. Archiving moves a project's folder off the machine into a Cloud Storage bucket and keeps the project listed, under a folded **Archived** group, so it can be restored. Deleting removes the folder without an archive. The design and its reasons are in [ADR 0006](adr/0006-project-archive.md).

- **The safe-removal check** runs before archiving or deleting, and again just before the folder goes. For every Git checkout it finds (as the Repos view does), it reports uncommitted changes, untracked files that are not ignored, stashes, local branches with commits on no remote branch, branches without an upstream, a detached HEAD with commits on no branch, commits reachable only from tags, notes or other refs, and linked worktrees outside the folder (whose repository the folder holds). Checkouts found inside a checkout are inspected the same way: checked-out submodules (a submodule repository in `.git/modules` that is not checked out is reported as unchecked), repositories inside ignored or untracked folders (`vendor/` holding a clone, say) and linked worktrees inside the folder. It also counts the files outside any checkout and their size, ignored files, special files (sockets, FIFOs, devices) and nested repositories it could not inspect. Git runs with the same hardened runner as the Repos view: no hooks, filters, fsmonitor or network. For a deletion, Git findings are blockers: commit and push, or remove them yourself first. For an archive they are warnings, because the archive keeps each checkout's `.git`, but those commits then live only in the archive, on no remote. The check's `fingerprint` changes whenever what it found changes; an archive or a deletion needs the fingerprint of the check you confirmed, and stops if the folder changed since.
- **Archiving** (`[archive]`, reference below) packs the folder into a tar.gz in a private temporary file (`$XDG_CACHE_HOME/kelpmux/archive`): symbolic links are stored as links and never followed, sockets, FIFOs and devices are skipped, modes and modification times are kept, and a mount point inside the folder stops it. The archive is uploaded as a new object `<prefix>/<project id>/<UTC time>.tar.gz` (never over an existing one) with the project's id, name, folder, KelpMux's version, the entries and bytes it unpacks to and the archive's SHA-256 as metadata. Cloud Storage checks the CRC32C KelpMux computed; KelpMux then reads the object's size, CRC32C and metadata back and compares them with its own, runs the check again (same fingerprint, and no file in the folder changed since packing began), records `status = "archived"` and the archive in `[project.archive]`, and only then removes the folder: renamed to a hidden `.kelpmux-removing-…` folder beside it, then deleted without following links. Nothing is removed if any step fails. The folder is never the files root, the projects folder, the Scratch folder, the home folder, a folder that holds another project, or outside `files.root` and `files.extra_roots`.
- **Restoring** lists the project's archives in the bucket, downloads the chosen one (the recorded one by default) to a private temporary file, checks its CRC32C against Cloud Storage's and its SHA-256 against the object's metadata (and the record's), and unpacks it into a fresh hidden folder beside the destination, never past the entries and bytes counted when it was packed (recorded in `[project.archive]` and the object's metadata; an archive without them gets at most 64 times its size plus 1 GiB) and never leaving less than 256 MiB free on that disk, refusing absolute paths, `..`, hard links, special files and anything through a symbolic link; the folder is then renamed into place. The destination is the original folder, which must not exist, or a new folder inside `projects.base`, which then becomes the project's folder. The project becomes active again and keeps its archive record. The archive stays in the bucket.
- **Deleting** removes the folder after the check, once you type the project's name, and optionally removes the project from the configuration. **Remove from KelpMux** removes only the project's table and keeps the folder (as removing the table by hand does).
- **Jobs**: archive, restore and delete run as `kelpmux project` processes of their own, so closing the browser does not stop them; the web client shows their progress and can cancel them until the folder starts being removed. At most one job per project runs at a time (`$XDG_STATE_HOME/kelpmux/archive/<id>.lock`); its state is `<id>.json` in the same folder. The web client refuses to archive or delete a project that still has open terminals.
- **Credentials**: the token for Cloud Storage comes from the GCE metadata server (the VM's service account), or from `archive.token_command`. It is kept in memory only, sent to `storage.googleapis.com` only, never through a proxy, and never shown, logged or sent to the browser. Settings, **Archive** shows where archives go and where the token comes from; **Test** writes and deletes a small object under `<prefix>/.kelpmux-test/`.
- Only Google Cloud Storage is supported. The storage code sits behind the archive module so that an S3-compatible backend (`backend = "s3"`, reserved) can be added later; see the roadmap.

### For agents

`kelpmux project` prints JSON (an object with `error` and exit status 1 on failure):

| Command | Does |
| --- | --- |
| `kelpmux project status ID [active\|paused]` | Prints the project's status, folder and archive record; with a status, sets it first. |
| `kelpmux project check-removal ID [--mode archive\|delete]` | Runs the safe-removal check (default mode: delete) and prints it, with its `fingerprint`. |
| `kelpmux project archive ID --fingerprint F` | Archives the project (the check must still give `F`), then removes its folder. |
| `kelpmux project archives ID` | Lists the project's archives in the bucket, newest first. |
| `kelpmux project restore ID [--object OBJECT] [--folder FOLDER]` | Restores the recorded archive (or `OBJECT`) into the original folder (or `FOLDER`, inside `projects.base`). |
| `kelpmux project delete ID --fingerprint F --name NAME [--remove]` | Deletes the folder after the check; `--remove` also removes the project. |
| `kelpmux project delete ID --keep-folder` | Removes the project from the configuration and keeps its folder. |
| `kelpmux project jobs` / `cancel ID` | Lists every project's last job / asks a running one to stop. |
| `kelpmux project archive-test` | Writes and deletes a small object in the bucket. |

Never archive or delete a project without the user's go-ahead: show them the check first.

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
| Applies | Immediately: the web client offers `<base>/<name>` for the next project it creates (and creates that folder). |

## `[scratch]`

Terminals outside any project.

### `scratch.folder`

The folder of the built-in Scratch project: terminals created outside any project start in it and belong to Scratch. It must lie inside `files.root` or one of `files.extra_roots`, and need not exist yet.

| | |
| --- | --- |
| Type | folder path (string) |
| Default | `"~/scratch"` |
| Applies | Immediately: the next terminal created outside a project starts there (the web client creates the folder when it is missing). |

## `[environments]`

Project environments: dev servers and deployments declared in a project's repositories (`.kelp/environments/`), started only on request from the web client or `kelpmux env`.

### `environments.ports`

The TCP ports given to environments as `KELP_PORT`: each (project, environment) gets the lowest port of this range that no other one holds and nothing listens on, and keeps it across restarts while it stays in the range. Two port numbers from 1024 to 65535, the first not above the second.

| | |
| --- | --- |
| Type | port range (string, `FIRST-LAST`) |
| Default | `"41000-41999"` |
| Applies | On the next start of an environment: one whose port falls outside a new range gets a new port. |

## `[github]`

GitHub in the web client: the repositories your GitHub account can see (Add repository on a project's Repos view), read-only facts about the GitHub repositories a project's checkouts come from, and cloning. The bridge reads the token this machine already has (the GitHub CLI, an environment variable or a command) and keeps it in memory only, for a few minutes; it never goes to the browser, a file or a log. Clones use your own Git credentials (SSH agent or credential helper). These keys are edited here only, never from the web client.

### `github.auth`

Where the GitHub token comes from: `"gh"` runs `gh auth token` (the GitHub CLI's sign-in), `"env"` reads `GH_TOKEN`, then `GITHUB_TOKEN`, from the web bridge's environment. Empty, with `github.token_command` empty too, leaves GitHub signed out: the repository list and the facts on the Repos view are off, cloning a URL still works. Set this or `github.token_command`, not both.

| | |
| --- | --- |
| Type | string |
| Default | `""` |
| Example | `auth = "gh"` |
| Applies | Immediately: the next GitHub request reads the token again. |

### `github.token_command`

A program that prints the GitHub token on its first line of output, run without arguments or a shell (for example a script that decrypts a SOPS-encrypted token). It must finish within 30 seconds and print at most 8 KiB; what it writes to its error output is never shown. Set this or `github.auth`, not both.

| | |
| --- | --- |
| Type | file path (string) |
| Default | `""` |
| Example | `token_command = "~/bin/github-token"` |
| Applies | Immediately: the next GitHub request runs it again. |

### `github.ssh_auth_sock`

The SSH agent socket Git uses to clone over SSH (`SSH_AUTH_SOCK`). Empty keeps the web bridge's own `SSH_AUTH_SOCK`, which a bridge started by the web relay's SSH connection usually lacks: point this at an agent socket that outlives your sessions for SSH clones to work from the web client.

| | |
| --- | --- |
| Type | file path (string) |
| Default | `""` |
| Example | `ssh_auth_sock = "~/.ssh/agent.sock"` |
| Applies | Immediately: the next clone uses it. |

## `[archive]`

Project archives: archiving packs a project's folder as a tar.gz, uploads it with a verified checksum, and only then removes the folder from this machine; restoring brings it back (see `kelpmux project` and ADR 0006). The machine's own Google identity is used, never a key file; the token stays in memory. These keys are edited here only, never from the web client.

### `archive.backend`

Where project archives go: `"gcs"` is Google Cloud Storage, through its JSON API. Empty turns archiving off (restoring too); deleting a project folder still works. S3-compatible storage is not supported yet; `"s3"` is reserved for it.

| | |
| --- | --- |
| Type | string |
| Default | `""` |
| Example | `backend = "gcs"` |
| Applies | Immediately: the next archive, restore or Test uses it. |

### `archive.bucket`

The bucket archives are written to and restored from: 3–63 lowercase letters, digits, `-`, `_` and `.`, starting and ending with a letter or digit. Required with `archive.backend`. The machine's identity needs to create, read, list and delete objects in it (on GCS, the Storage Object User role on the bucket).

| | |
| --- | --- |
| Type | string |
| Default | `""` |
| Example | `bucket = "my-kelpmux-archives"` |
| Applies | Immediately. |

### `archive.prefix`

The folder of the bucket archives go in: each is `<prefix>/<project id>/<UTC time>.tar.gz`, and Test writes and deletes one small object under `<prefix>/.kelpmux-test/`. Letters, digits, `-`, `_`, `.` and `/` between non-empty parts, at most 256 bytes; no `..`.

| | |
| --- | --- |
| Type | string |
| Default | `"kelpmux"` |
| Applies | Immediately: archives already made keep their names (a restore lists the current prefix). |

### `archive.token_command`

A program that prints a Google OAuth access token on its first line of output, run without arguments or a shell (a script that runs `gcloud auth print-access-token`, say). Empty uses the machine's own service account through the GCE metadata server, the usual choice on a Google Cloud VM. It must finish within 30 seconds and print at most 8 KiB; its error output is never shown. The token is kept in memory only and sent to `storage.googleapis.com` only.

| | |
| --- | --- |
| Type | file path (string) |
| Default | `""` |
| Example | `token_command = "~/bin/gcs-token"` |
| Applies | Immediately: the next archive operation reads the token again. |

## `[[project]]`

Projects: one `[[project]]` table each, with a name and a folder. Every terminal belongs to one project; terminals created outside any project belong to the built-in Scratch project (`scratch.folder`), which is not listed here. The web client adds, renames and deletes projects here too.

```toml
[[project]]
id = "kelp"
name = "Kelp"
folder = "~/projects/Kelp"
color = "teal"
status = "active"
```

### `project.id`

A stable identifier: lowercase letters, digits and `-`, at most 64, starting with a letter or digit; unique, and not `scratch`. Terminals are linked to their project by it, so keep it when renaming. The web client makes one from the name.

| | |
| --- | --- |
| Type | string |
| Required | yes |
| Example | `id = "kelp"` |
| Applies | Immediately. |

### `project.name`

The name shown: at most 64 characters, without `/` or control characters, unique regardless of case, and not `Scratch`.

| | |
| --- | --- |
| Type | string |
| Required | yes |
| Example | `name = "Kelp"` |
| Applies | Immediately. |

### `project.folder`

The project's folder. It must lie inside `files.root` or one of `files.extra_roots`, and may not be inside, or contain, another project's folder or `scratch.folder`. It need not exist: a project whose folder is missing shows as missing. Deleting a project never deletes its folder.

| | |
| --- | --- |
| Type | folder path (string) |
| Required | yes |
| Example | `folder = "~/projects/Kelp"` |
| Applies | Immediately. |

### `project.color`

Optional: the colour that marks the project in the sidebar and the file browser, one of red, orange, yellow, green, teal, blue, purple, pink, gray, or `#rrggbb`.

| | |
| --- | --- |
| Type | string |
| Required | no |
| Example | `color = "teal"` |
| Applies | Immediately. |

### `project.status`

Optional: `"active"` (the default), `"paused"` (shown dimmed in the sidebar, nothing else changes) or `"archived"` (its folder was archived and removed from this machine; the project stays listed so it can be restored). Archiving and restoring set it; set `"paused"` or `"active"` by hand or from the web client.

| | |
| --- | --- |
| Type | string |
| Required | no |
| Example | `status = "active"` |
| Applies | Immediately. |

### `project.archive`

Written by KelpMux, not by hand: the last archive made of the project (`[project.archive]`), with its bucket, object, size, SHA-256, CRC32C and time, and the entries and bytes it unpacks to. Restoring uses it by default, never unpacks more than those counts, and keeps the record afterwards.

| | |
| --- | --- |
| Type | table |
| Required | no |
| Example | `archive = { backend = "gcs", bucket = "my-kelpmux-archives", object = "kelpmux/kelp/20261124T100000Z.tar.gz", size = 1048576, sha256 = "…", crc32c = "AAAAAA==", archived_at = "2026-11-24T10:00:00Z", entries = 1200, unpacked_bytes = 4194304 }` |
| Applies | Immediately. |

<!-- END kelpmux config docs -->
