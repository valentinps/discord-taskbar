#!/usr/bin/env bash
#
# Publish a GitHub release of the current version.
#
# Installed copies update from the latest release's `discord-taskbar.exe`, so
# that asset's name is part of the updater's contract (see `update_source` in
# crates/app/src/main.rs). The installer rides along for new installs.
#
# Bump `version` in the workspace Cargo.toml and commit before running this.

set -o pipefail
cd "$(dirname "$0")" || exit 1

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
TAG="v$VERSION"

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "Uncommitted changes - commit first, so the tag is the code that shipped."
    exit 1
fi
if gh release view "$TAG" >/dev/null 2>&1; then
    echo "$TAG is already released - bump the version in Cargo.toml."
    exit 1
fi

./make-installer.sh || exit 1

git tag -a "$TAG" -m "Discord Taskbar $VERSION" || exit 1
git push origin "$TAG" || exit 1

gh release create "$TAG" \
    --title "Discord Taskbar $VERSION" \
    --notes "Download **DiscordTaskbarSetup.exe** to install. Installed copies update themselves." \
    dist/DiscordTaskbarSetup.exe \
    target/release/discord-taskbar.exe || exit 1

echo "Released $TAG"
