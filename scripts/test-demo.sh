#!/usr/bin/env bash
# First-party shell fixtures only; never start a real terminal or workspace.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/scripts" "$fixture/examples" "$fixture/target/demos/aaaaaaa" "$fixture/target/demos/bbbbbbb"
cp "$repo/scripts/demo.sh" "$fixture/scripts/demo.sh"
printf '{}\n' > "$fixture/examples/native-tabs.json"
for build in aaaaaaa bbbbbbb; do
  cat > "$fixture/target/demos/$build/kelpmux" <<'MOCK'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$XDG_STATE_HOME/arguments"
MOCK
  chmod +x "$fixture/target/demos/$build/kelpmux"
done
export XDG_STATE_HOME="$fixture/state"
mkdir -p "$XDG_STATE_HOME"
ln -s aaaaaaa "$fixture/target/demos/current"
# It must work outside the repository and initialize only a new workspace.
(cd / && bash "$fixture/scripts/demo.sh")
grep -Fx 'demo-aaaaaaa' "$XDG_STATE_HOME/arguments"
grep -Fx -- '--config' "$XDG_STATE_HOME/arguments"
mkdir -p "$XDG_STATE_HOME/kelpmux/workspaces/demo-aaaaaaa"
printf '{}\n' > "$XDG_STATE_HOME/kelpmux/workspaces/demo-aaaaaaa/workspace.json"
bash "$fixture/scripts/demo.sh"
if grep -Fxq -- '--config' "$XDG_STATE_HOME/arguments"; then exit 1; fi
# Selecting another build preserves the old workspace and its configuration.
ln -sfn bbbbbbb "$fixture/target/demos/current"
bash "$fixture/scripts/demo.sh" --takeover
grep -Fx 'demo-bbbbbbb' "$XDG_STATE_HOME/arguments"
grep -Fx -- '--takeover' "$XDG_STATE_HOME/arguments"
test -f "$XDG_STATE_HOME/kelpmux/workspaces/demo-aaaaaaa/workspace.json"
rm "$XDG_STATE_HOME/arguments"
bash "$fixture/scripts/demo.sh" --print-command > "$fixture/preview"
test ! -e "$XDG_STATE_HOME/arguments"
grep -F 'demo-bbbbbbb' "$fixture/preview"
ln -sfn "$fixture" "$fixture/target/demos/current"
if bash "$fixture/scripts/demo.sh" --print-command 2>/dev/null; then exit 1; fi
