#!/usr/bin/env bash
# Run only in publish, after the matrix has created a draft release and before
# uploading latest.json or making it public. A draft may not have a Git ref.
set -euo pipefail
VERSION=$(node -p "require('./package.json').version")
TAG="app-v${VERSION}"
BUILT_SHA=$(git rev-parse HEAD)
if [ "$BUILT_SHA" != "${GITHUB_SHA:?GITHUB_SHA is required}" ]; then
  echo "Refusing to publish ${TAG}: checkout=${BUILT_SHA}, run=${GITHUB_SHA}" >&2
  exit 1
fi

# Existing tag: never push over it, only check its identity. Absent tag: create
# a lightweight ref at the approved build commit. A non-force push refuses a
# concurrent different tag; release concurrency also serializes our runs.
if git ls-remote --exit-code origin "refs/tags/${TAG}" >/dev/null; then
  : # A draft may already have made the ref; verification below decides.
else
  result=$?
  if [ "$result" -ne 2 ]; then
    echo "Cannot determine whether ${TAG} exists (git exit ${result}); refusing to create a tag" >&2
    exit "$result"
  fi
  git push origin "${GITHUB_SHA}:refs/tags/${TAG}"
fi
# FETCH_HEAD^{commit} peels both lightweight and annotated remote tags.
git fetch --no-tags origin "refs/tags/${TAG}"
TAG_SHA=$(git rev-parse 'FETCH_HEAD^{commit}')
if [ "$TAG_SHA" != "$BUILT_SHA" ]; then
  echo "Refusing to publish ${TAG}: tag=${TAG_SHA}, build=${BUILT_SHA}" >&2
  exit 1
fi
echo "Verified ${TAG} => ${BUILT_SHA}"
