#!/usr/bin/env bash
# The CHANGELOG.md section for VERSION, as the release's notes.
#
#   scripts/release-notes.sh 0.3.0
set -euo pipefail
version=$1
notes=$(awk -v v="$version" '
  index($0, "## [" v "]") == 1 { on = 1; next }
  on && /^## \[/ { exit }
  on { print }
' "$(dirname "$0")/../CHANGELOG.md")
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
  echo "CHANGELOG.md has no section for $version" >&2
  exit 1
fi
printf '%s\n' "$notes" | sed -e '/./,$!d'
