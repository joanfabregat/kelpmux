#!/usr/bin/env bash
# Stable SSH entry for explicitly promoted, immutable demo builds.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
demos="$repo/target/demos"
build=$(readlink -f -- "$demos/current") || {
  echo 'No validated demo has been selected yet.' >&2
  exit 1
}
id=${build##*/}
if [[ ${build%/*} != "$demos" || ! $id =~ ^[a-f0-9]{7,40}$ || ! -x "$build/kelpmux" ]]; then
  echo 'The selected demo must be an executable in target/demos/BUILD_ID.' >&2
  exit 1
fi
workspace="demo-$id"
state_base=${XDG_STATE_HOME:-}
[[ $state_base == /* ]] || state_base="$HOME/.local/state"
args=("$build/kelpmux" ssh-entry --workspace "$workspace")
# Existing workspaces retain settings changed through the configuration panel.
if [[ ! -f "$state_base/kelpmux/workspaces/$workspace/workspace.json" ]]; then
  args+=(--config "$repo/examples/native-tabs.json")
fi
inspect=false
for arg in "$@"; do
  case "$arg" in
    --print-command) inspect=true ;;
    --takeover) args+=(--takeover) ;;
    *) echo 'Usage: demo.sh [--takeover] [--print-command]' >&2; exit 2 ;;
  esac
done
if $inspect; then
  printf '%q ' "${args[@]}"
  printf '\n'
  exit 0
fi
printf 'Kelpmux demo %s · workspace %s\n' "$id" "$workspace" >&2
exec "${args[@]}"
