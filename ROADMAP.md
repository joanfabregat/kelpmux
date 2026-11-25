# Roadmap

KelpMux is an open-source tool to pilot a machine (a VM, a container or a pod) from the browser and run coding agents and anything else on it. A managed offering (VMs deployed with KelpMux preinstalled and configured) may come later. KelpHub is stopped; the parts worth keeping move here.

This file lists open work only. Done work is in the Git history and the changelog. Items within a section are roughly in order.

## In progress

- **History fixes**: Resume only for known conversations (an explicit picker otherwise), one row for a terminal with one conversation, matching unknown Codex conversations by folder and time.
- **GitHub with the machine's own auth** (`gh`, an environment token or a token command): repository picker, cloning into a project, read-only repository state.
- **Environments**: an ADR and a first version of `process` and `deploy` environments per project.

## Terminal server

- Record when each terminal opens and detect Codex conversation ids reliably, so History never has to match by folder and time.
- Raise the crates' `rust-version` to 1.89 and fix the let-chain Clippy lints this enables.
- Forward every bridge notice through the shared browser worker, then remove the 5 s config and project polling.

## Imported from KelpHub

- **GitHub App**: a public KelpMux App with device-flow sign-in (scoped access, no key on the machine), and a token broker for the managed offering.
- **GitHub pull requests**: read-only list of a repository's pull requests with checks and reviews, in the Repos view and the project page.
- **Repository history**: branches, log, commit details and file diffs in the Repos view, with revisions validated before use.
- **Worktrees**: discover a repository's worktrees and start an agent in a checkout or a new worktree, one per agent.
- **Safe removal**: before deleting a checkout or folder, list everything not safely on a remote; re-check before deleting.
- **Agent marks**: a finished turn stays highlighted until seen in any browser; per-project roll-ups (check what KelpMux already has first).
- **Environments** (KelpHub ADR 0004): `process` and `deploy` environments declared in a repository, started and stopped explicitly from the UI or by agents with `kelpmux env up|down|status|logs`, each instance with a slug, a stable port and a state folder.
- **Project instructions**: `.kelp/AGENTS.md` in a configuration repository, with `AGENTS.md` and a `CLAUDE.md` pointer generated at the project root.
- **Permanent web deployment**: channels, exact HTTPS origin, SSH host-key verification, pinned artifacts and supervision, as the base of a managed VM.

## New features

- **Project status and archive**: active, paused and archived projects; archiving packs the project folder as tar.gz, uploads it to GCS or S3-compatible storage with a verified checksum, then removes it from the machine after the safe-removal check. Restore from an archive.
- **Runtime backends** (KelpHub ADR 0006): one interface over the Docker Engine API (Docker, Podman's socket or any compatible engine) and Kubernetes, replacing the Podman-specific tooling on the host.
- **KelpMux in a container or a pod**: an image with sshd, the terminal server and the bridge, the home folder on a volume; SSH stays the way in. Explore a second way in through an API (token or passkey to the relay, bridge on a local socket) later.
- **Dev and staging deployments**: environments run in containers through the runtime backend, and a KelpMux reverse proxy routes `<slug>.<domain>` to each running instance, as the wildcard domain does on jbox today.
- **Review working changes**: a diff of uncommitted changes per file from the Repos view.
- **Scratch terminals**: optional fresh folder per terminal under the Scratch folder, never deleted automatically.

## Loose ends

- Prettier formatting across `web/` and a CI check for it.
- Cmd/Ctrl-click on paths that wrap across lines.
- A WebKit drag-and-drop test for uploads.
- An ADR for the Files view (browser, editor, tabs, columns).
- Device checks of the mobile UI on real phones (kelpmux#110).
- The web client replaces the TUI pre-prod on jbox once it reaches parity; the TUI pre-prod stays untouched until then.
