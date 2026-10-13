# Release procedure

The native development version is `0.2.0-alpha.1`. A version change prepares code for a release; it does not publish a tag, install anything or start a server. Native trials should use a dedicated socket and state directory while existing tmux workspaces continue independently.

1. Ensure main CI is green, including native PTY/client tests on Linux x86-64 and ARM64, Rust 1.85, RustSec and license checks, and legacy tmux compatibility tests.
2. Confirm `Cargo.toml` and `Cargo.lock` contain the intended version; commit the version change and update the documentation if needed.
3. Create a signed `vVERSION` tag and push it when authorized. The tag workflow validates the repository and builds GNU/Linux x86-64 and ARM64 archives and SHA-256 checksum artifacts.
4. Publish the matching GitHub release when authorized. The published-release workflow repeats validation/build and attaches the archives and checksums. It does not publish to crates.io, install a binary or start a server.
5. Wait for every release-build job to pass before calling the release ready.
6. For a native trial, verify the checksum and unpack into a chosen location. Start `kelpmux server` explicitly with separate socket/state paths, then `attach`. Exercise ANSI output and Unicode, detaching/reconnecting with the same shell PID, controller/observer takeover, reading/fullscreen/close, resize and occupied-pane shrink refusal before using real agents.

A manual `Release build` dispatch validates packaging without creating a tag or publishing a release. Actions artifacts expire after 14 days; attached GitHub release assets remain on the release. x86-64 archives build on Ubuntu 22.04 and ARM64 on Ubuntu 24.04. The binaries require compatible GNU/Linux glibc; macOS and Windows server builds are outside this alpha's scope.

Each archive contains the Kelpmux binary, Apache-2.0 license and notice, README, contribution guide, assets, docs, examples, scripts and `dependency-licenses/` with locked versions and upstream license texts. Rust dependencies retain their own license terms, including the Unicode data license where applicable. Native operation requires no tmux, and archives do not bundle it. Users retaining legacy compatibility commands provide their own tmux 3.8+; the repository's checksum-verifying `scripts/build-tmux.sh` remains available for an explicit private build.

Release smoke tests must use disposable shells, private socket/state paths and explicit shutdown. Detach preserves owned processes while the server stays alive; shutdown ends them. The seven-day journal is metadata only and is not a backup or process-recovery mechanism. Never test a release by replacing a running user server or modifying its configuration.
