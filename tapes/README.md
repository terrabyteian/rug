# Demo tapes

[VHS](https://github.com/charmbracelet/vhs) scripts that record the GIFs
embedded in the main README. Each `demo-*.tape` renders `docs/demo-*.gif`.

## Re-recording

```bash
brew install vhs        # one-time
bash scripts/record-demos.sh
```

The script builds the release binary, resets `fixtures/`, records every tape
(read-only ones first, state-mutating ones with a fixture reset in between),
and prints the resulting GIF sizes, warning above 2 MB.

To re-record a single tape, reproduce the same steps from the repo root —
tapes assume the current directory is the repo root and use
`./target/release/rug`:

```bash
cargo build --release
./fixtures/reset.sh --keep-init
vhs tapes/demo-cached-plan.tape
./fixtures/reset.sh --keep-init   # after demo-hero/run/cached-plan/targeted
```

## Conventions

- Shared settings: Dracula theme, FontSize 14, 1200×720 px (≈117×36 cells —
  keeps the TUI in its richest layout tier: all board columns plus the
  `P:{age}` badge need ≥110 columns, ≥30 rows).
- `Hide`/`Show` wraps setup that shouldn't appear in the GIF (the `rug`
  alias, cursor positioning, waiting out slow stretches).
- Completion is detected with `Wait+Screen /regex/` rather than fixed sleeps
  wherever possible. Note the Run-screen header counts *modules*, not tasks —
  a second task on the same module never bumps `✓N`, so wait on output text
  (e.g. `/Apply complete/`) instead.
- `services/big-state` must never run init/plan (state is hand-crafted, no
  module sources); `remote-state/demo` needs MinIO. Keep both out of any
  plan fan-out — the hero tape unmarks them off-camera.
