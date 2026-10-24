#!/usr/bin/env bash
# Check that every new non-merge commit follows Conventional Commits, using
# committed.toml at the repository root.
#
# Usage: check-commits.sh <base> <head>
#   Checks the commits reachable from <head> and not from <base>. Merge
#   commits are skipped. When <base> is all zeros or not in the clone (a new
#   branch, or a force push that dropped it), only <head> itself is checked.
set -euo pipefail

base=$1
head=$2
zero=0000000000000000000000000000000000000000

if [[ "$base" == "$zero" ]] || ! git cat-file -e "${base}^{commit}" 2>/dev/null; then
  echo "Base ${base} is not available; checking ${head} only."
  commits=$(git rev-list --no-merges --max-count=1 "$head")
else
  commits=$(git rev-list --no-merges --reverse "${base}..${head}")
fi

if [[ -z "$commits" ]]; then
  echo "No non-merge commits to check."
  exit 0
fi

failed=0
for commit in $commits; do
  subject=$(git log -1 --format=%s "$commit")
  if committed --config committed.toml "$commit"; then
    echo "ok   ${commit:0:7} ${subject}"
  else
    echo "FAIL ${commit:0:7} ${subject}"
    failed=1
  fi
done

if [[ $failed -ne 0 ]]; then
  echo
  echo "Some commits are not Conventional Commits: 'type(scope)!: description',"
  echo "with type one of the allowed_types in committed.toml."
  echo "See https://www.conventionalcommits.org/en/v1.0.0/"
fi
exit "$failed"
