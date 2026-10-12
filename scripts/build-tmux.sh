#!/bin/sh
# Build a verified upstream tmux release into an explicit, unprivileged prefix.
# Run offline in the project's approved build container, with TMPDIR on disk.
set -eu

if [ "$#" -ne 2 ]; then
    echo 'usage: build-tmux.sh TMUX_ARCHIVE INSTALL_PREFIX' >&2
    exit 64
fi
archive=$(realpath "$1")
prefix=$(realpath -m "$2")
version=3.8
checksum=e79c699c7e949dccd0a4a125e17b8d1261e311b16979dbc8eb34542f3966d82e
printf '%s  %s\n' "$checksum" "$archive" | sha256sum -c -
tmux_build_dir=$(mktemp -d "${TMPDIR:-/tmp}/kelpmux-tmux-XXXXXXXX")
trap 'rm -rf "$tmux_build_dir"' EXIT HUP INT TERM
tar -xzf "$archive" -C "$tmux_build_dir"
cd "$tmux_build_dir/tmux-$version"
./configure --prefix="$prefix" --disable-utf8proc CFLAGS='-O2'
make -j "${TMUX_BUILD_JOBS:-2}"
mkdir -p "$prefix/bin" "$prefix/share/doc/tmux" "$prefix/share/man/man1"
install -m 755 tmux "$prefix/bin/tmux"
strip "$prefix/bin/tmux"
install -m 644 COPYING "$prefix/share/doc/tmux/COPYING"
install -m 644 tmux.1 "$prefix/share/man/man1/tmux.1"
printf 'tmux %s\nsource-sha256 %s\n' "$version" "$checksum" > "$prefix/share/doc/tmux/build-source.txt"
"$prefix/bin/tmux" -V
