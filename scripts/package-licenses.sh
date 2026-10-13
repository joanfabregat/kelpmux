#!/usr/bin/env bash
# Collect license texts from the exact locked Cargo source packages.
set -euo pipefail
out=${1:?usage: package-licenses.sh OUTPUT_DIRECTORY}
mkdir -p "$out"
cargo metadata --locked --offline --format-version 1 > "$out/metadata.json"
jq '[.packages[] | {name, version, license, repository}]' "$out/metadata.json" > "$out/dependencies.json"
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
done < <(jq -r '.packages[] | select(.name != "kelpmux") | [.name, .version, .manifest_path] | @tsv' "$out/metadata.json")
rm "$out/metadata.json"
