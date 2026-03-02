#!/usr/bin/env bash
set -euo pipefail

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY_RUN=true
  echo "==> Dry-run mode: builds will run but no tag/push/release will happen"
fi

# ---------------------------------------------------------------------------
# 1. Parse version from Cargo.toml
# ---------------------------------------------------------------------------
VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
TAG="v${VERSION}"
echo "==> Version: ${VERSION}  Tag: ${TAG}"

# ---------------------------------------------------------------------------
# 2. Guard: must be on master with a clean tree
# ---------------------------------------------------------------------------
BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [[ "$BRANCH" != "master" ]]; then
  echo "ERROR: must be on master branch (currently on '${BRANCH}')" >&2
  exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "ERROR: working tree is not clean — commit or stash changes first" >&2
  exit 1
fi

if ! $DRY_RUN && git rev-parse "$TAG" &>/dev/null; then
  echo "ERROR: tag ${TAG} already exists" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 3. Check prerequisites
# ---------------------------------------------------------------------------
for cmd in cargo zig gh; do
  if ! command -v "$cmd" &>/dev/null; then
    echo "ERROR: '${cmd}' not found on PATH" >&2
    echo "  Run: brew install zig  (for zig)" >&2
    echo "       cargo install cargo-zigbuild  (for zigbuild)" >&2
    echo "       brew install gh  (for GitHub CLI)" >&2
    exit 1
  fi
done

if ! command -v cargo-zigbuild &>/dev/null; then
  echo "ERROR: cargo-zigbuild not installed — run: cargo install cargo-zigbuild" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 4. Build
# ---------------------------------------------------------------------------
DIST="dist"
rm -rf "$DIST"
mkdir -p "$DIST"

echo "==> Building aarch64-apple-darwin (native)"
cargo build --release --target aarch64-apple-darwin

echo "==> Building x86_64-unknown-linux-gnu"
cargo zigbuild --release --target x86_64-unknown-linux-gnu

echo "==> Building aarch64-unknown-linux-gnu"
cargo zigbuild --release --target aarch64-unknown-linux-gnu

# ---------------------------------------------------------------------------
# 5. Package .tar.gz archives
# ---------------------------------------------------------------------------
package() {
  local target="$1"
  local archive_name="$2"
  local binary="target/${target}/release/rug"

  if [[ ! -f "$binary" ]]; then
    echo "ERROR: binary not found at ${binary}" >&2
    exit 1
  fi

  tar -czf "${DIST}/${archive_name}" -C "$(dirname "$binary")" "$(basename "$binary")"
  echo "    created ${DIST}/${archive_name}"
}

echo "==> Packaging archives"
package "aarch64-apple-darwin"       "rug-${TAG}-darwin-arm64.tar.gz"
package "x86_64-unknown-linux-gnu"   "rug-${TAG}-linux-x86_64.tar.gz"
package "aarch64-unknown-linux-gnu"  "rug-${TAG}-linux-arm64.tar.gz"

if $DRY_RUN; then
  echo "==> Dry-run complete. Archives in ${DIST}/:"
  ls -lh "$DIST/"
  exit 0
fi

# ---------------------------------------------------------------------------
# 6. Tag and push
# ---------------------------------------------------------------------------
echo "==> Tagging ${TAG}"
git tag "$TAG"

echo "==> Pushing tag"
git push origin "$TAG"

# ---------------------------------------------------------------------------
# 7. Create GitHub Release with auto-generated notes and upload assets
# ---------------------------------------------------------------------------
echo "==> Creating GitHub Release ${TAG}"
gh release create "$TAG" \
  --title "rug ${TAG}" \
  --generate-notes \
  "${DIST}"/*.tar.gz

echo "==> Done! https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/${TAG}"
