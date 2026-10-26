#!/usr/bin/env bash
# First-party command fixtures; no real workspace, shell job, or installation.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
export XDG_DATA_HOME="$fixture/data with spaces"
export XDG_STATE_HOME="$fixture/state"
mkdir -p "$XDG_STATE_HOME"
manager="$repo/scripts/channels.sh"
cat > "$fixture/kelpmux" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$XDG_STATE_HOME/arguments"
printf '%s\n' "${KELPMUX_DEV_BADGE:-unset}" > "$XDG_STATE_HOME/badge"
printf '%s ' "$@" >> "$XDG_STATE_HOME/calls"
printf '\n' >> "$XDG_STATE_HOME/calls"
if [[ $1 == start ]]; then
  test ! -e "/proc/$$/fd/9"
  if flock -n "$XDG_DATA_HOME/kelpmux/channels/lock" true; then exit 1; fi
elif [[ $1 == ssh-entry ]]; then
  flock -n "$XDG_DATA_HOME/kelpmux/channels/lock" true
fi
MOCK
chmod +x "$fixture/kelpmux"
for build in aaaaaaa bbbbbbb; do
  bash "$manager" register "$build" "$fixture/kelpmux" "$repo/examples/native-tabs.json"
done
bash "$manager" register aaaaaaa "$fixture/kelpmux" "$repo/examples/native-tabs.json"
bash "$manager" install-manager aaaaaaa
manager="$XDG_DATA_HOME/kelpmux/channel-manager"
bash "$manager" select dev aaaaaaa
bash "$manager" promote preprod
(cd / && bash "$manager" launch preprod)
grep -Fx preprod "$XDG_STATE_HOME/arguments"
grep -Fx 0 "$XDG_STATE_HOME/badge"
grep -Fx -- --config "$XDG_STATE_HOME/arguments"
mkdir -p "$XDG_STATE_HOME/kelpmux/workspaces/preprod"
printf '{"tab_count":4}\n' > "$XDG_STATE_HOME/kelpmux/workspaces/preprod/workspace.json"
bash "$manager" select dev bbbbbbb
rm "$XDG_STATE_HOME/calls"
bash "$manager" launch dev --takeover
grep -F 'start --workspace dev --replace-running --server-binary' "$XDG_STATE_HOME/calls"
grep -Fx dev "$XDG_STATE_HOME/arguments"
grep -Fx 1 "$XDG_STATE_HOME/badge"
grep -Fx -- --takeover "$XDG_STATE_HOME/arguments"
KELPMUX_DEV_BADGE=0 bash "$manager" launch dev
grep -Fx 0 "$XDG_STATE_HOME/badge"
if KELPMUX_DEV_BADGE=invalid bash "$manager" launch dev --print-command 2>/dev/null; then exit 1; fi
# Promoting changes only the selection. It cannot invoke the binary, stop jobs,
# overwrite workspace settings, or create build-specific workspaces.
rm "$XDG_STATE_HOME/arguments"
bash "$manager" promote preprod
test ! -e "$XDG_STATE_HOME/arguments"
jq -e '.tab_count == 4' "$XDG_STATE_HOME/kelpmux/workspaces/preprod/workspace.json" >/dev/null
bash "$manager" launch preprod
if tail -n 1 "$XDG_STATE_HOME/calls" | grep -Fq -- '--replace-running'; then exit 1; fi
if grep -Fxq -- --config "$XDG_STATE_HOME/arguments"; then exit 1; fi
test "$(jq -r .current "$XDG_DATA_HOME/kelpmux/channels/preprod.json")" = bbbbbbb
test "$(jq -r .previous "$XDG_DATA_HOME/kelpmux/channels/preprod.json")" = aaaaaaa
bash "$manager" rollback preprod
test "$(jq -r .current "$XDG_DATA_HOME/kelpmux/channels/preprod.json")" = aaaaaaa
test "$(jq -r .current "$XDG_DATA_HOME/kelpmux/channels/dev.json")" = bbbbbbb
rm "$XDG_STATE_HOME/arguments"
bash "$manager" status > "$fixture/status"
grep -F 'preprod: selected=aaaaaaa' "$fixture/status"
bash "$manager" launch preprod --print-command > "$fixture/command"
test ! -e "$XDG_STATE_HOME/arguments"
grep -F -- '--workspace preprod' "$fixture/command"
bash "$manager" launch dev --print-command > "$fixture/dev-command"
test ! -e "$XDG_STATE_HOME/arguments"
grep -F -- '--replace-running' "$fixture/dev-command"
bash "$manager" stop dev
printf 'stop\n--workspace\ndev\n' > "$fixture/expected"
cmp "$fixture/expected" "$XDG_STATE_HOME/arguments"
# Re-registering altered artifacts and selecting unregistered/unsafe paths fail.
printf '# changed\n' >> "$fixture/kelpmux"
if bash "$manager" register aaaaaaa "$fixture/kelpmux" "$repo/examples/native-tabs.json" 2>/dev/null; then exit 1; fi
if bash "$manager" select preprod aaaaaaa 2>/dev/null; then exit 1; fi
if bash "$manager" select dev ccccccc 2>/dev/null; then exit 1; fi
if bash "$manager" select dev ../aaaaaaa 2>/dev/null; then exit 1; fi
if bash "$manager" launch preprod --workspace other 2>/dev/null; then exit 1; fi
if bash "$manager" status invalid 2>/dev/null; then exit 1; fi
# A relative XDG path falls back to HOME, rather than the caller's directory.
HOME="$fixture/clean-home" XDG_DATA_HOME=relative bash "$manager" status 2>/dev/null && exit 1
printf 'Channel launcher contracts passed.\n'
