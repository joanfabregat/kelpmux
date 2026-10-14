#!/usr/bin/env bash
# Collect license texts from every exact locked source package, including platforms
# other than the build host. Run cargo fetch --locked first (without --target).
set -euo pipefail
out=${1:?usage: package-licenses.sh OUTPUT_DIRECTORY}
mkdir -p "$out"
trap 'rm -f "$out/metadata.json" "$out/packages.tsv"' EXIT
cargo metadata --locked --offline --format-version 1 > "$out/metadata.json"
jq '[.packages[] | {name, version, license, repository}]' "$out/metadata.json" > "$out/dependencies.json"
# A separate checked jq command propagates parsing failures; process substitution
# would otherwise hide a failure and silently create an incomplete distribution.
jq -er '.packages[] | select(.name != "kelpmux") | [.name, .version, .manifest_path] | @tsv' "$out/metadata.json" > "$out/packages.tsv"
while IFS=$'\t' read -r name version manifest; do
  dir=${manifest%/*}
  dest="$out/$name-$version"
  mkdir -p "$dest"
  found=0
  for file in "$dir"/LICENSE* "$dir"/LICENCE* "$dir"/COPYING* "$dir"/COPYRIGHT* "$dir"/NOTICE* "$dir"/license* "$dir"/licence* "$dir"/copying* "$dir"/copyright* "$dir"/notice*; do
    if [[ -f "$file" ]]; then
      cp "$file" "$dest/"
      found=1
    fi
  done
  if [[ "$found" != 1 ]]; then
    printf 'Missing packaged license text for %s %s\n' "$name" "$version" >&2
    exit 1
  fi
done < "$out/packages.tsv"
