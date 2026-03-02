# Release scripts

## Prerequisites (one-time setup)

```bash
brew install zig
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
brew install gh   # GitHub CLI — run `gh auth login` if not already authenticated
```

## Doing a release

```bash
# 1. Bump version in Cargo.toml (e.g. 0.1.0 → 0.2.0)
# 2. Commit the bump:
git add Cargo.toml Cargo.lock && git commit -m "chore: bump version to 0.2.0"
# 3. Run the release script:
bash scripts/release.sh
```

That's it. The script will:
- Build native macOS arm64 + cross-compiled Linux x86_64 and arm64 binaries
- Package each as a `.tar.gz` in `dist/`
- Create and push git tag `v{VERSION}`
- Create a GitHub Release with auto-generated notes and upload all three archives

## Dry run (test builds without publishing)

```bash
bash scripts/release.sh --dry-run
```

Runs all three builds and creates the archives in `dist/`, but skips tagging,
pushing, and creating the GitHub Release.

## Artifact naming

| File | Target |
|------|--------|
| `rug-vX.Y.Z-darwin-arm64.tar.gz` | `aarch64-apple-darwin` |
| `rug-vX.Y.Z-linux-x86_64.tar.gz` | `x86_64-unknown-linux-gnu` |
| `rug-vX.Y.Z-linux-arm64.tar.gz` | `aarch64-unknown-linux-gnu` |

Each archive contains a single `rug` binary.
