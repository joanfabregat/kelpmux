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
grep -Fx -- --config "$XDG_STATE_HOME/arguments"
mkdir -p "$XDG_STATE_HOME/kelpmux/workspaces/preprod"
printf '{"tab_count":4}\n' > "$XDG_STATE_HOME/kelpmux/workspaces/preprod/workspace.json"
bash "$manager" select dev bbbbbbb
bash "$manager" launch dev --takeover
grep -Fx dev "$XDG_STATE_HOME/arguments"
grep -Fx -- --takeover "$XDG_STATE_HOME/arguments"
# Promoting changes only the selection. It cannot invoke the binary, stop jobs,
# overwrite workspace settings, or create build-specific workspaces.
rm "$XDG_STATE_HOME/arguments"
bash "$manager" promote preprod
test ! -e "$XDG_STATE_HOME/arguments"
jq -e '.tab_count == 4' "$XDG_STATE_HOME/kelpmux/workspaces/preprod/workspace.json" >/dev/null
bash "$manager" launch preprod
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
