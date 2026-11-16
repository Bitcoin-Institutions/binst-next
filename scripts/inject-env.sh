#!/usr/bin/env bash
# reused-from: binst-pilot-webapp@dda2d1a:scripts/inject-env.sh (2026-11-16) — adapted: none (verbatim)
# inject-env.sh — substitute __REOWN_PROJECT_ID__ in the *built* index.html.
#
# Runs as a Trunk post_build hook (see Trunk.toml), so it edits the copy in
# Trunk's staging directory and never the tracked index.html:
#   source .env && trunk build      (or: trunk serve)
#
# Can also be run by hand after a build: bash scripts/inject-env.sh [dist dir]

set -euo pipefail

dir="${TRUNK_STAGING_DIR:-${1:-dist}}"
target="$dir/index.html"

if [[ ! -f "$target" ]]; then
  echo "[inject-env] $target not found; run trunk build first" >&2
  exit 1
fi

if [[ -z "${REOWN_PROJECT_ID:-}" ]]; then
  echo "[inject-env] REOWN_PROJECT_ID is not set; WalletConnect stays disabled. Copy .env.example → .env to enable it." >&2
  exit 0
fi

# Portable in-place edit (BSD and GNU sed)
sed -i.bak "s|__REOWN_PROJECT_ID__|${REOWN_PROJECT_ID}|g" "$target"
rm -f "$target.bak"

echo "[inject-env] REOWN_PROJECT_ID injected into $target"
