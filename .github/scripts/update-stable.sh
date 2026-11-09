#!/usr/bin/env bash
set -euo pipefail

# Resolve Latest on every run, including recovery and delayed release events.
latest() {
  gh api "repos/$GITHUB_REPOSITORY/releases/latest" \
    --jq 'select(.draft == false and .prerelease == false) | .tag_name'
}
tag=$(latest)
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo 'Latest is not a stable version tag.' >&2; exit 1; }
git fetch origin "refs/tags/$tag"
commit=$(git rev-parse 'FETCH_HEAD^{commit}')
# Another release may have been published while the fetch was running.
[[ "$tag" == "$(latest)" ]] || { echo 'Latest changed; rerun this workflow.' >&2; exit 1; }
# Pass authentication only to this push; checkout credentials stay disabled.
# An empty helper prevents a runner's configured helper from taking precedence.
git -c credential.helper= -c 'credential.helper=!gh auth git-credential' \
  push "https://github.com/$GITHUB_REPOSITORY.git" "$commit:refs/heads/stable"
