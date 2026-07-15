#!/usr/bin/env bash
#
# reset.sh — restore fixtures/ to a pristine state.
#
# Running the TUI (or headless CLI) against fixtures/ during development
# leaves the tree in one of two broken states:
#
#   1. "Stuck applied" — tracked terraform.tfstate baselines (e.g.
#      fixtures/legacy/monolith/terraform.tfstate) diverge from the
#      committed baseline after a real apply, so subsequent plans show
#      "No changes" instead of exercising the plan path.
#
#   2. Stale ".terraform/" backend cache — a module that was init'd
#      against one backend config (or an older provider lock) starts
#      demanding `init -reconfigure` / `-migrate-state` before it will
#      plan again.
#
# On top of that, ad-hoc runs scatter *.tfstate.backup / lock-info files
# under fixtures/.state/ that need clearing without destroying the one
# fixture that's supposed to live there permanently: a *fake* lock file
# (fixtures/.state/apps-api.tfstate.lock.info) used to test the
# force-unlock keybinding. This script regenerates that file verbatim
# after any cleanup that would otherwise delete it.
#
# Usage:
#   fixtures/reset.sh [--keep-init] [--dry-run] [--help]
#
#   (no flags)     Full reset: restore tracked files, delete all
#                  ignored/untracked runtime cruft under fixtures/
#                  (including .terraform/ caches), regenerate the fake
#                  lock. Modules will need re-init afterwards.
#
#   --keep-init    Restore tracked files and clear fixtures/.state/
#                  only. .terraform/ directories are left alone, so
#                  modules stay initialised and don't need re-init.
#
#   --dry-run      Print what would happen; change nothing. Composable
#                  with --keep-init.
#
# Not handled here: fixtures/remote-state/demo keeps its state in a
# MinIO bucket (docker compose, fixtures/remote-state/). This script
# can't reach into that container; see the printed note.

set -euo pipefail

KEEP_INIT=0
DRY_RUN=0

usage() {
  cat <<'EOF'
Usage: fixtures/reset.sh [--keep-init] [--dry-run] [--help]

Restore fixtures/ to a pristine state after TUI/CLI testing.

  (no flags)     Full reset: restore tracked files to HEAD, delete all
                 ignored/untracked runtime artifacts under fixtures/
                 (including .terraform/ backend caches), regenerate the
                 fake force-unlock fixture. Re-init required afterwards.

  --keep-init    Restore tracked files and clear fixtures/.state/ only.
                 .terraform/ directories are preserved, so modules stay
                 initialised (no re-init needed).

  --dry-run      Show what would happen without changing anything.
                 Composable with --keep-init.

  --help         Show this message.

Does not touch fixtures/remote-state/demo's MinIO-backed state — see
the printed note for that module's reset procedure.
EOF
}

for arg in "$@"; do
  case "$arg" in
    --keep-init) KEEP_INIT=1 ;;
    --dry-run) DRY_RUN=1 ;;
    --help|-h) usage; exit 0 ;;
    *)
      echo "reset.sh: unknown argument: $arg" >&2
      usage >&2
      exit 1
      ;;
  esac
done

ROOT="$(git rev-parse --show-toplevel)"
FIXTURES="$ROOT/fixtures"

if [[ ! -d "$FIXTURES" ]]; then
  echo "reset.sh: $FIXTURES does not exist, refusing to run" >&2
  exit 1
fi

STATE_DIR="$FIXTURES/.state"
FAKE_LOCK="$STATE_DIR/apps-api.tfstate.lock.info"

# Exact byte content of the fake lock fixture used to test the force-unlock
# (U) keybinding. Kept here so it survives any cleanup of fixtures/.state/.
read -r -d '' FAKE_LOCK_CONTENT <<'EOF' || true
{
  "ID": "deadbeef-1234-5678-abcd-000000000000",
  "Operation": "OperationTypePlan",
  "Info": "",
  "Who": "ian@devbox",
  "Version": "1.6.0",
  "Created": "2024-03-01T12:00:00.000000Z",
  "Path": ""
}
EOF

regenerate_fake_lock() {
  mkdir -p "$STATE_DIR"
  printf '%s\n' "$FAKE_LOCK_CONTENT" > "$FAKE_LOCK"
}

print_remote_state_note() {
  cat <<EOF

Note: fixtures/remote-state/demo keeps its state in a MinIO bucket, not
on the filesystem, so this script cannot reset it. To reset that module:

  cd fixtures/remote-state
  docker compose down -v
  cd demo
  tofu init -reconfigure
EOF
}

if [[ "$DRY_RUN" -eq 1 ]]; then
  echo "=== dry run: tracked files that would be restored ==="
  git -C "$ROOT" diff --name-only -- fixtures/ || true
  git -C "$ROOT" diff --name-only --cached -- fixtures/ || true

  if [[ "$KEEP_INIT" -eq 1 ]]; then
    echo
    echo "=== dry run: --keep-init would delete everything under fixtures/.state/ ==="
    if [[ -d "$STATE_DIR" ]]; then
      find "$STATE_DIR" -mindepth 1
    else
      echo "(fixtures/.state/ does not exist)"
    fi
    echo
    echo "=== dry run: fake lock file would be regenerated at ==="
    echo "$FAKE_LOCK"
    echo "=== dry run: .terraform/ caches would be PRESERVED ==="
  else
    echo
    echo "=== dry run: ignored files that would be deleted (git clean -ndX) ==="
    git -C "$ROOT" clean -ndX -- fixtures/
    echo
    echo "=== dry run: fake lock file would be regenerated at ==="
    echo "$FAKE_LOCK"
  fi

  print_remote_state_note
  exit 0
fi

echo "Restoring tracked fixture files to HEAD..."
git -C "$ROOT" checkout -- fixtures/

if [[ "$KEEP_INIT" -eq 1 ]]; then
  echo "Clearing fixtures/.state/ (preserving .terraform/ caches)..."
  if [[ -d "$STATE_DIR" ]]; then
    find "$STATE_DIR" -mindepth 1 -delete
  fi
  regenerate_fake_lock
  echo
  echo "Done (--keep-init): tracked files restored, fixtures/.state/ cleared,"
  echo "fake lock regenerated. .terraform/ caches were left in place, so"
  echo "modules remain initialised."
else
  echo "Removing ignored runtime artifacts under fixtures/ (.terraform/, .state/ contents, backups)..."
  # Capital -X: delete ignored files ONLY. Never lowercase -x or plain -f,
  # which would also sweep up untracked-but-intended new fixtures.
  git -C "$ROOT" clean -fdX -- fixtures/
  regenerate_fake_lock
  echo
  echo "Done (full reset): tracked files restored, ignored runtime artifacts"
  echo "removed (including .terraform/ backend caches), fake lock regenerated."
  echo
  echo "Modules now need re-initialising before they'll plan:"
  echo "  cargo run -- --dir fixtures/ init --all"
  echo "  (or: tofu init / terraform init in an individual module directory)"
fi

print_remote_state_note
