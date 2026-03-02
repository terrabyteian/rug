# rug test fixtures

A realistic nested terraform structure for testing `rug` locally.
Uses the `null` provider — no cloud credentials required.

## Structure

```
fixtures/
├── infra/
│   ├── vpc/        root module (has backend block)
│   ├── eks/        root module (has backend block)
│   └── modules/
│       └── networking/   library module (no backend — excluded by default)
└── apps/
    ├── api/        root module (has backend block)
    └── web/        root module (has backend block)
```

State files are written to `fixtures/.state/` (local backend).

## Running

```bash
# List discovered modules
cargo run -- list --dir fixtures/

# TUI mode
cargo run -- --dir fixtures/

# Headless plan across all modules
cargo run -- plan --all --dir fixtures/

# Headless plan with filter
cargo run -- plan --filter vpc --dir fixtures/

# Show library modules too
cargo run -- --dir fixtures/ --show-library
```

## Initialising

Each module needs `terraform init` before `plan`/`apply`:

```bash
cargo run -- init --all --dir fixtures/
```
