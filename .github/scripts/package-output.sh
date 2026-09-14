#!/usr/bin/env bash
set -euo pipefail
# Run after the output build and repository verification, on the pinned Linux runner.
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]]
sha=$(git rev-parse HEAD)
[[ $sha =~ ^[a-f0-9]{40}$ ]]
epoch=$(git show -s --format=%ct HEAD)
source_binary=${1:-target/release/music-output}
output=${2:-dist/music-output}
"$source_binary" --version
mkdir -p "$output"
output=$(cd "$output" && pwd)
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
install -m 0755 "$source_binary" "$stage/music-output"
install -m 0644 clients/headless/music-output.service "$stage/"
# The download remains readable outside a source checkout.
sed "s|(\.\./README\.md)|(https://github.com/pjunak/music/blob/$sha/clients/README.md)|g" \
  clients/headless/README.md > "$stage/README.md"
printf '%s\n' "$sha" > "$stage/REVISION"
chmod 0644 "$stage/README.md" "$stage/REVISION"
# Stable metadata keeps reruns of an unchanged binary byte-identical.
tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner \
  -C "$stage" -cf - music-output REVISION README.md music-output.service | \
  gzip -n > "$output/music-output-linux-x86_64.tar.gz"
cp "$stage/REVISION" "$output/REVISION"
(cd "$output" && sha256sum music-output-linux-x86_64.tar.gz > SHA256SUMS)
