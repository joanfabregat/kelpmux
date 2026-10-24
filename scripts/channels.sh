#!/usr/bin/env bash
# Local immutable builds and stable, independent native workspace channels.
set -euo pipefail
umask 077
fail() { printf '%s\n' "$*" >&2; exit 1; }
usage() {
  cat <<'HELP'
Usage: kelpmux-channels launch dev|preprod [--takeover] [--print-command]
       kelpmux-channels status
       kelpmux-channels stop dev|preprod  (ends that workspace's jobs)
       kelpmux-channels register BUILD_ID BINARY DEFAULT_CONFIG
       kelpmux-channels select dev BUILD_ID
       kelpmux-channels promote preprod
       kelpmux-channels rollback dev|preprod
       kelpmux-channels install-manager BUILD_ID
Promotion selects the next server build. Running workspaces are never restarted.
HELP
}
xdg_data=${XDG_DATA_HOME:-}
[[ $xdg_data == /* ]] || xdg_data="$HOME/.local/share"
xdg_state=${XDG_STATE_HOME:-}
[[ $xdg_state == /* ]] || xdg_state="$HOME/.local/state"
root="$xdg_data/kelpmux"
channel_ok() { [[ $1 == dev || $1 == preprod ]] || fail 'Channel must be dev or preprod.'; }
build_ok() { [[ $1 =~ ^[a-f0-9]{7,40}$ ]] || fail 'Build ID must be a Git commit hash (7–40 lowercase hex characters).'; }
build_path() {
  build_ok "$1"
  local path="$root/builds/$1"
  [[ -d $path && ! -L $path && -x $path/kelpmux && ! -L $path/kelpmux && -f $path/defaults.json && -f $path/build.json ]] || fail "Build $1 is not registered."
  [[ $(readlink -f -- "$path") == "$path" ]] || fail 'Build paths must not contain symlinks.'
  printf '%s\n' "$path"
}
selected() {
  channel_ok "$1"
  jq -er '.current | select(type == "string")' "$root/channels/$1.json" || fail "Channel $1 has no selected build."
}
lock() {
  mkdir -p "$root/channels" "$root/builds"
  exec 9>"$root/channels/lock"
  flock -x 9
}
# One atomic JSON replacement commits both current and previous pointers.
select_build() {
  local channel=$1 next=$2 previous=null temp
  channel_ok "$channel"
  build_path "$next" >/dev/null
  if [[ -f $root/channels/$channel.json ]]; then
    previous=$(jq -er '.current' "$root/channels/$channel.json")
    [[ $previous != "$next" ]] || { printf '%s already selects %s\n' "$channel" "$next"; return; }
  fi
  temp=$(mktemp "$root/channels/.selection-XXXXXXXX")
  jq -n --arg current "$next" --arg previous "$previous" '{current:$current,previous:(if $previous == "null" then null else $previous end)}' > "$temp"
  mv -Tf -- "$temp" "$root/channels/$channel.json"
  printf '%s now selects %s; running workspaces are unchanged.\n' "$channel" "$next"
}
[[ $# -gt 0 ]] || { usage; exit 2; }
command=$1
shift
case "$command" in
  launch)
    [[ $# -ge 1 ]] || fail 'Missing channel.'
    channel=$1; shift
    channel_ok "$channel"
    inspect=false
    args=()
    for arg in "$@"; do
      case "$arg" in
        --print-command) inspect=true ;;
        --takeover) args+=(--takeover) ;;
        *) fail "Unsupported launch option: $arg" ;;
      esac
    done
    build=$(selected "$channel")
    path=$(build_path "$build")
    launch=("$path/kelpmux" ssh-entry --workspace "$channel")
    if [[ ! -f $xdg_state/kelpmux/workspaces/$channel/workspace.json ]]; then
      launch+=(--config "$path/defaults.json")
    fi
    launch+=("${args[@]}")
    if $inspect; then printf '%q ' "${launch[@]}"; printf '\n'; exit; fi
    printf 'KelpMux %s · selected build %s · reconnect preserves the running server\n' "$channel" "$build" >&2
    exec "${launch[@]}"
    ;;
  register)
    [[ $# == 3 ]] || fail 'register requires BUILD_ID BINARY DEFAULT_CONFIG.'
    build=$1; binary=$2; config=$3
    build_ok "$build"
    [[ -f $binary && -x $binary ]] || fail 'Binary must be an executable regular file.'
    jq -e '.tab_count == 1' "$config" >/dev/null || fail 'Channel defaults must start with one shell.'
    lock
    target="$root/builds/$build"
    if [[ -e $target || -L $target ]]; then
      build_path "$build" >/dev/null
      cmp -s -- "$binary" "$target/kelpmux" && cmp -s -- "$config" "$target/defaults.json" && cmp -s -- "${BASH_SOURCE[0]}" "$target/channels.sh" || fail 'Build already exists with different contents; immutable builds cannot be overwritten.'
      printf 'Build %s already registered.\n' "$build"
      exit
    fi
    stage=$(mktemp -d "$root/builds/.register-XXXXXXXX")
    trap 'rm -rf -- "$stage"' EXIT
    install -m 555 -- "$binary" "$stage/kelpmux"
    install -m 444 -- "$config" "$stage/defaults.json"
    install -m 555 -- "${BASH_SOURCE[0]}" "$stage/channels.sh"
    digest=$(sha256sum "$stage/kelpmux" | cut -d ' ' -f 1)
    jq -n --arg commit "$build" --arg sha256 "$digest" '{commit:$commit,sha256:$sha256}' > "$stage/build.json"
    mv -T -- "$stage" "$target"
    trap - EXIT
    printf 'Registered immutable build %s\n' "$build"
    ;;
  stop)
    [[ $# == 1 ]] || fail 'stop requires a channel and ends its shells and jobs.'
    channel_ok "$1"
    path=$(build_path "$(selected "$1")")
    exec "$path/kelpmux" stop --workspace "$1"
    ;;
  select)
    [[ $# == 2 && $1 == dev ]] || fail 'Use select dev BUILD_ID; preprod accepts promotion from dev only.'
    lock
    select_build dev "$2"
    ;;
  promote)
    [[ $# == 1 && $1 == preprod ]] || fail 'Use promote preprod.'
    lock
    select_build preprod "$(selected dev)"
    ;;
  rollback)
    [[ $# == 1 ]] || fail 'rollback requires a channel.'
    channel_ok "$1"
    lock
    previous=$(jq -er '.previous | select(type == "string")' "$root/channels/$1.json") || fail 'No previous build is available.'
    select_build "$1" "$previous"
    ;;
  install-manager)
    [[ $# == 1 ]] || fail 'install-manager requires BUILD_ID.'
    lock
    path=$(build_path "$1")
    temp=$(mktemp -d "$root/.manager-XXXXXXXX")
    trap 'rmdir -- "$temp"' EXIT
    ln -s "$path/channels.sh" "$temp/channel-manager"
    mv -Tf -- "$temp/channel-manager" "$root/channel-manager"
    ;;
  status)
    [[ $# == 0 ]] || fail 'status takes no arguments.'
    for channel in dev preprod; do
      build=$(selected "$channel")
      build_path "$build" >/dev/null
      previous=$(jq -r '.previous // "none"' "$root/channels/$channel.json")
      printf '%s: selected=%s previous=%s\n' "$channel" "$build" "$previous"
      identity="$xdg_state/kelpmux/workspaces/$channel/server.identity.json"
      pid=$(jq -er '.pid | select(type == "number" and . > 0)' "$identity" 2>/dev/null) || pid=0
      birth=$(jq -r '.birth' "$identity" 2>/dev/null) || birth=0
      actual_birth=$(awk '{sub(/^.*\) /, "");print $20}' "/proc/$pid/stat" 2>/dev/null) || actual_birth=missing
      saved_exe=$(jq -r '.executable' "$identity" 2>/dev/null) || saved_exe=missing
      actual_exe=$(readlink "/proc/$pid/exe" 2>/dev/null) || actual_exe=missing
      if [[ $pid != 0 && $birth == "$actual_birth" && $saved_exe == "$actual_exe" ]]; then
        printf '  running: build=%s pid=%s protocol=%s\n' "$(basename "$(dirname "$actual_exe")")" "$pid" "$(jq -r '.protocol' "$identity")"
      else
        printf '  stopped (starts on your next launch)\n'
      fi
    done
    ;;
  --help|-h|help) usage ;;
  *) usage >&2; exit 2 ;;
esac
