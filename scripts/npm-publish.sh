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
	# A path, not a module name: require needs it absolute.
	name=$(node -p 'require(require("path").resolve(process.argv[1])).name' "$dir/package.json")
	version=$(node -p 'require(require("path").resolve(process.argv[1])).version' "$dir/package.json")
	if npm view "$name@$version" version >/dev/null 2>&1; then
		echo "skip $name@$version: already published"
		continue
	fi
	(cd "$dir" && npm publish --access public "$@")
	echo "published $name@$version"
done
