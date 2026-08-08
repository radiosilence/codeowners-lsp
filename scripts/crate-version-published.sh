#!/usr/bin/env bash
# Is <crate> <version> already on crates.io?
#
# Exits 0 if published, 1 if not. Queries the sparse index rather than the web
# API: the API rejects unauthenticated automated traffic under its data-access
# policy, and a rejection is indistinguishable from "crate absent" — which
# would make the publish step think it had work to do every single run.
set -euo pipefail

crate="$1"
version="$2"

# Sparse index shards by name length: ab/cd/name for names of 4+ chars.
case ${#crate} in
  1) path="1/$crate" ;;
  2) path="2/$crate" ;;
  3) path="3/${crate:0:1}/$crate" ;;
  *) path="${crate:0:2}/${crate:2:2}/$crate" ;;
esac

body=$(curl -sS --fail-with-body "https://index.crates.io/$path" 2>/dev/null) || {
  echo "$crate is not on crates.io yet"
  exit 1
}

# One JSON object per line, one per published version.
if jq -e --arg v "$version" 'select(.vers == $v)' <<<"$body" >/dev/null; then
  exit 0
fi

echo "$crate $version not found on crates.io (latest: $(jq -r '.vers' <<<"$body" | tail -3 | tr '\n' ' '))"
exit 1
