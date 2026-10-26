#!/bin/sh
# Publishes the packages scripts/npm-packages.mjs assembled in OUT: the
# platform packages first, so @solcreek/linear-cli never points at a version
# that is not there yet. A package whose version is already published is
# skipped, so a rerun after a partial failure finishes the job.
# Extra arguments go to npm publish (e.g. --otp CODE).
set -eu
out=${1:?usage: scripts/npm-publish.sh OUT [npm publish args]}
shift
for dir in "$out"/linear-cli-* "$out"/linear-cli; do
	name=$(node -p "require('$dir/package.json').name")
	version=$(node -p "require('$dir/package.json').version")
	if npm view "$name@$version" version >/dev/null 2>&1; then
		echo "skip $name@$version: already published"
		continue
	fi
	(cd "$dir" && npm publish --access public "$@")
	echo "published $name@$version"
done
