#!/usr/bin/env bash
# Clone upstream boring.notch at the commit these patches were cut from, apply the
# WARDEN patches, and (unless --no-install) build and install it to /Applications.
#
# Usage: integrations/boring-notch/setup.sh [--no-install] [DEST]
#   DEST defaults to ../boring.notch next to this repo.
#   UPSTREAM overrides the clone source (a local clone works too).

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
UPSTREAM="${UPSTREAM:-https://github.com/TheBoredTeam/boring.notch.git}"
BASE="99900bf630a3d3e97fae079df2175993318d51f7"
BRANCH="warden-section"

INSTALL=1
if [ "${1:-}" = "--no-install" ]; then INSTALL=0; shift; fi
DEST="${1:-$HERE/../../../boring.notch}"

if [ -d "$DEST/.git" ]; then
  echo "==> Using existing clone at $DEST"
  if git -C "$DEST" rev-parse --verify -q "$BRANCH" >/dev/null; then
    echo "Branch $BRANCH already exists in $DEST. Not touching it."
    echo "Delete the branch or pick another DEST to start clean."
    exit 1
  fi
  git -C "$DEST" fetch -q origin
else
  echo "==> Cloning $UPSTREAM into $DEST"
  git clone -q "$UPSTREAM" "$DEST"
fi

# -B: a fresh clone of a fork can already carry this branch as its default. An
# existing clone with the branch was refused above, so nothing of the user's is reset.
git -C "$DEST" checkout -q -B "$BRANCH" "$BASE"
echo "==> Applying $(ls "$HERE"/patches/*.patch | wc -l | tr -d ' ') patches onto ${BASE:0:7}"
git -C "$DEST" am -q --committer-date-is-author-date "$HERE"/patches/*.patch

if [ "$INSTALL" = 1 ]; then
  "$DEST/scripts/build-and-install.sh"
else
  echo "Patched. Build and install with: $DEST/scripts/build-and-install.sh"
fi
