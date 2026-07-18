# Agent notes for rug

rug is a terraform/tofu multiplexer: single-crate Rust binary (`rug`), TUI +
headless CLI. Default branch is **master** (not main).

- When changing user-visible behavior (commands, flags, key bindings, config
  fields, install process, supported platforms), update `README.md` — user
  docs only, no internal implementation details there.

## Release process

Full mechanics live in `scripts/README.md`; `scripts/release.sh` drives
everything. The short version:

1. Bump `version` in `Cargo.toml`, let `Cargo.lock` follow (a `cargo build`
   or `cargo update -p rug` updates it), commit both
   (`chore: bump version to X.Y.Z`), push `master`.
2. `bash scripts/release.sh` — parses the version from `Cargo.toml`, refuses
   to run off `master` or with a dirty tree, builds darwin-arm64 natively and
   both Linux targets via `cargo zigbuild`, packages `dist/*.tar.gz`, tags
   `v<version>`, pushes the tag, creates the GitHub release with
   `--generate-notes` and the three archives.
   - `--dry-run`: build + package only.
   - `--assets-only v<X.Y.Z>`: rebuild and re-upload archives to an existing
     release (recovery path for wrong/missing assets).

Do not release by hand or rename artifacts: `install.sh` reconstructs
`rug-<tag>-<os>-<arch>.tar.gz` (`darwin-arm64`, `linux-x86_64`,
`linux-arm64`) to build its download URL, so the naming is load-bearing.

### Toolchain (host: darwin-arm64)

- `zig` (homebrew) + `cargo-zigbuild` do the Linux cross-builds; requires
  rustup targets `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`.
  No cross/Docker on this machine.
- `cargo-zigbuild` lives in `~/.cargo/bin`; non-interactive shells may need
  `export PATH="$HOME/.cargo/bin:$PATH"` before running the script.
- Git identity: global config is unset on this machine — if committing fails
  with "Author identity unknown", set repo-local
  `git config user.name "Ian Hall"` / `git config user.email
  terrabytian@gmail.com`.
