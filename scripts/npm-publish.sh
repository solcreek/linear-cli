#!/bin/sh
# Publishes the packages scripts/npm-packages.mjs assembled in OUT: the
# platform packages first, then @solcreek/linear-cli once npm serves every
# one of them. The registry's CDN lags a publish by a minute or so, and npm
# skips an optional dependency it cannot resolve without a word, so a main
# package published at once installs without its binary for that minute
# (seen with 0.2.0). A package whose version is already published is
# skipped, so a rerun after a partial failure finishes the job.
# Extra arguments go to npm publish (e.g. --dry-run).
set -eu
out=${1:?usage: scripts/npm-publish.sh OUT [npm publish args]}
shift
dry=false
for a in "$@"; do
	case $a in --dry-run) dry=true ;; esac
done

field() { node -p 'require(require("path").resolve(process.argv[1]))[process.argv[2]]' "$1/package.json" "$2"; }

# publish DIR [npm publish args]
publish() {
	dir=$1
	shift
	name=$(field "$dir" name)
	version=$(field "$dir" version)
	if npm view "$name@$version" version >/dev/null 2>&1; then
		echo "skip $name@$version: already published"
		return
	fi
	(cd "$dir" && npm publish --access public "$@")
	echo "published $name@$version"
}

# served: is NAME@VERSION in the document npm install reads?
served() {
	curl -fsS -H 'Accept: application/vnd.npm.install-v1+json' -H 'Cache-Control: no-cache' \
		"https://registry.npmjs.org/$(printf %s "$1" | sed 's|/|%2f|')" 2>/dev/null |
		node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>process.exit(JSON.parse(s).versions?.[process.argv[1]]?0:1))' "$2"
}

for dir in "$out"/linear-cli-*; do
	publish "$dir" "$@"
done
if [ "$dry" = false ]; then
	for dir in "$out"/linear-cli-*; do
		name=$(field "$dir" name)
		version=$(field "$dir" version)
		tries=0
		until served "$name" "$version"; do
			tries=$((tries + 1))
			if [ "$tries" -gt 60 ]; then
				echo "$name@$version is not served after 10 minutes; not publishing the main package" >&2
				exit 1
			fi
			sleep 10
		done
		echo "served $name@$version"
	done
fi
publish "$out/linear-cli" "$@"
