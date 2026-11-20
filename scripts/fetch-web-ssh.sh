#!/usr/bin/env bash
# Fetch only; do not execute downloaded code.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../web" && pwd)
revision=7fbbe3166e501763fa7e18309544beecaccd78f4
plugin_sha=c89d8f185031a6649adc28411b912dbf3ca8392684fdfc115addbf477cc4719b
temp_base=${TMPDIR:-/tmp}
if [[ -z ${TMPDIR:-} && -d /data/agent-tmp ]]; then temp_base=/data/agent-tmp; fi
work=$(mktemp -d -p "$temp_base" agent-XXXXXXXX)
printf 'Download workspace: %s\n' "$work"
trap 'rm -rf -- "$work"' EXIT

git init -q "$work/libapps"
git -C "$work/libapps" fetch -q --depth=1 https://chromium.googlesource.com/apps/libapps "$revision"
git -C "$work/libapps" checkout -q --detach FETCH_HEAD
test "$(git -C "$work/libapps" rev-parse HEAD)" = "$revision"
curl --fail --silent --show-error --location --proto '=https' \
  https://storage.googleapis.com/chromeos-localmirror/secureshell/releases/0.82.tar.xz \
  --output "$work/plugin.tar.xz"
printf '%s  %s\n' "$plugin_sha" "$work/plugin.tar.xz" | sha256sum -c -
mkdir -p "$root/public/vendor/libapps/wassh" "$root/public/vendor/libapps/wasi-js-bindings"
cp -R "$work/libapps/wassh/js" "$root/public/vendor/libapps/wassh/"
cp -R "$work/libapps/wasi-js-bindings/js" "$root/public/vendor/libapps/wasi-js-bindings/"
cp "$work/libapps/wasi-js-bindings/index.js" "$root/public/vendor/libapps/wasi-js-bindings/"
cp "$work/libapps/LICENSE" "$root/public/vendor/libapps/"
tar -xJf "$work/plugin.tar.xz" -C "$work" plugin/wasm/ssh.wasm
cp "$work/plugin/wasm/ssh.wasm" "$root/public/vendor/ssh.wasm"
printf 'WASSH %s and verified OpenSSH WASM release 0.82 fetched.\n' "$revision"
