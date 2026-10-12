# Release procedure

The repository is prepared for `0.1.0-beta.2`; creating this version string does not create a release. Do not install or start a beta in a user's workspace without their agreement.

1. Ensure main CI is green and review the discovery limitations and supported platforms.
2. Confirm `Cargo.toml` and `Cargo.lock` contain the intended version; commit any version change.
3. Create a signed tag `vVERSION` and push it. The tag workflow validates the code and builds native Linux x86-64 and ARM64 archives and SHA-256 checksums as Actions artifacts.
4. Publish the matching GitHub release when authorized. The published-release workflow repeats validation/build and attaches the archives and checksums. It does not publish to crates.io or install anything.
5. Wait for all release-build jobs to pass before calling the release ready.
6. For the first trial, inspect the archive/checksum, preserve the existing tmux configuration, and review the generated fragment before choosing bindings. Start discovery only after approval, check known session IDs against agent TUIs, then exercise native reading/fullscreen/restore and layout refusal with disposable shell panes.

A manual `Release build` dispatch validates packaging without creating a tag or publishing a release. Actions artifacts expire after 14 days; attached GitHub release assets remain on the release. x86-64 archives build on Ubuntu 22.04, ARM64 on Ubuntu 24.04; these are GNU/Linux binaries and require compatible glibc. The release includes the project's Apache-2.0 license and notice; Rust dependencies retain their own license terms.

The native preview archives also contain an unmodified tmux 3.8 build under `tmux/`, its upstream license, manual and SHA-256 source digest. `scripts/build-tmux.sh` verifies the published upstream source checksum before compilation. The C build needs libevent and ncurses development packages; the resulting binary needs compatible libevent and libtinfo runtime libraries. Use the bundled executable explicitly with `--tmux-binary` and an isolated socket/state directory when testing alongside apt's tmux. Publishing remains an explicit release operation; the development version on main is not itself a published beta.
