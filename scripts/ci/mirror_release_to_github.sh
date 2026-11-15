#!/usr/bin/env bash
# Mirror a Gitea release tarball into the GitHub repository as one commit.
# Required environment: GITEA_URL GITEA_REPO GITEA_TOKEN GH_TOKEN GH_REPO [GH_BRANCH] TAG
set -euo pipefail

: "${GITEA_URL:?GITEA_URL is required}"
: "${GITEA_REPO:?GITEA_REPO is required}"
: "${GITEA_TOKEN:?GITEA_TOKEN is required}"
: "${GH_TOKEN:?GH_TOKEN is required}"
: "${GH_REPO:?GH_REPO is required}"
: "${TAG:?TAG is required}"
GH_BRANCH=${GH_BRANCH:-main}

GH_REPO=${GH_REPO#https://github.com/}
GH_REPO=${GH_REPO#http://github.com/}
GH_REPO=${GH_REPO#git@github.com:}
GH_REPO=${GH_REPO%.git}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

api="${GITEA_URL%/}/api/v1/repos/${GITEA_REPO}/releases/tags/${TAG}"
auth=(-H "Authorization: token ${GITEA_TOKEN}")

asset_url=""
json=$(curl -fsSL "${auth[@]}" "$api" 2>/dev/null || true)
if [ -n "$json" ]; then
    asset_url=$(printf '%s' "$json" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for a in d.get("assets", []):
    name = a.get("name", "")
    if name.endswith((".tar.gz", ".tgz")):
        print(a.get("browser_download_url", ""))
        break
' 2>/dev/null || true)
fi

if [ -z "$asset_url" ]; then
    asset_url="${GITEA_URL%/}/api/v1/repos/${GITEA_REPO}/archive/${TAG}.tar.gz"
fi

echo "downloading release archive"
if [ -n "${RELEASE_TARBALL:-}" ]; then
    cp "$RELEASE_TARBALL" "$work/release.tar.gz"
else
    curl -fsSL "${auth[@]}" -o "$work/release.tar.gz" "$asset_url"
fi

mkdir -p "$work/src"
tar -xzf "$work/release.tar.gz" -C "$work/src"
src=$(find "$work/src" -mindepth 1 -maxdepth 1 -type d | head -n 1)
if [ -z "$src" ]; then
    echo "release archive has no top-level directory" >&2
    exit 1
fi

repo_url=${GH_REPO_URL:-"https://x-access-token:${GH_TOKEN}@github.com/${GH_REPO}.git"}
if ! git clone --quiet --branch "$GH_BRANCH" "$repo_url" "$work/gh" 2>/dev/null; then
    rm -rf "$work/gh"
    mkdir -p "$work/gh"
    git -C "$work/gh" init --quiet
    git -C "$work/gh" remote add origin "$repo_url"
    if git -C "$work/gh" fetch --quiet origin "$GH_BRANCH" 2>/dev/null; then
        git -C "$work/gh" checkout --quiet -B "$GH_BRANCH" FETCH_HEAD
    fi
fi

cd "$work/gh"
git checkout -B "$GH_BRANCH" --quiet
find . -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
tar -C "$src" -cf - . | tar -xf -
# never publish the private instance's CI configuration to GitHub
rm -rf .gitea

git config user.name "release-mirror"
git config user.email "release-mirror@users.noreply.github.com"
git add -A
if git diff --cached --quiet; then
    echo "no changes for ${TAG}, nothing to push"
    exit 0
fi
git commit --quiet -m "sync: ${TAG}"
git push --quiet origin "HEAD:${GH_BRANCH}"
echo "mirrored ${TAG} to ${GH_REPO}:${GH_BRANCH}"
