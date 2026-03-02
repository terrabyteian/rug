# rug test fixtures

A realistic nested terraform structure for testing `rug` locally.
Uses the `null` provider — no cloud credentials required.

## Structure

```
fixtures/
├── infra/
│   ├── vpc/          root module
│   ├── eks/          root module
│   └── modules/
│       └── networking/   library module (excluded by default)
├── apps/
│   ├── api/          root module  (has a fake lock file for testing force-unlock)
│   └── web/          root module
├── services/
│   ├── slow-plan/    plan takes ~10s  (external data source)
│   ├── slow-plan-2/  plan takes ~40s  (3 parallel external checks + 4 resources)
│   ├── slow-deploy/  apply takes ~15s
│   ├── verbose-deploy/
│   └── all-operations/
└── remote-state/
    ├── docker-compose.yml   MinIO S3-compatible server
    └── demo/                S3 remote state + locking + slow plan (~30s)
```

State files for local-backend modules are written to `fixtures/.state/`.

## Running

```bash
# List discovered modules
cargo run -- --dir fixtures/ list

# TUI mode
cargo run -- --dir fixtures/

# Headless plan across all modules
cargo run -- --dir fixtures/ plan --all

# Headless plan with filter
cargo run -- --dir fixtures/ plan --filter vpc

# Show library modules too
cargo run -- --dir fixtures/ --show-library list
```

## Initialising

Most modules are pre-initialised (`.terraform/` committed). For new modules
that need `tofu init` run locally:

```bash
# slow-plan-2 (needs external + null providers downloaded)
cd fixtures/services/slow-plan-2 && tofu init

# Or init everything at once
cargo run -- --dir fixtures/ init --all
```

## Remote state via MinIO

`fixtures/remote-state/` contains a MinIO S3-compatible server and a demo
module that uses it as a Terraform S3 backend. No AWS account needed.

State locking uses OpenTofu 1.7+ native S3 locking (`use_lockfile = true`),
which writes a `.tflock` object to the bucket — no DynamoDB required.

### Setup

```bash
# 1. Start MinIO (creates the rug-fixtures bucket automatically)
cd fixtures/remote-state
docker compose up -d

# 2. Initialise the module
cd demo
tofu init -reconfigure

# 3. Run from rug
cargo run -- --dir fixtures/remote-state/
```

MinIO web console: http://localhost:9001 (minioadmin / minioadmin)

### Testing lock cancellation

The demo module has a ~30s slow plan (`data "external" "config_check"`).
To produce a stale lock:

1. In rug, select `remote-state/demo` and press `p` to start a plan
2. While the plan is running (status = ⟳), press `C` and confirm to cancel
3. The process is killed mid-run, leaving `terraform.tfstate.tflock` in MinIO
4. The next plan attempt will fail with "Error acquiring the state lock"
   showing the Lock ID in the output pane
5. Run `tofu force-unlock -force <LOCK_ID>` from the command line to clear it
   (rug's `U` keybinding covers local-backend locks; S3 lock detection is a
   future addition)

### Teardown

```bash
cd fixtures/remote-state
docker compose down          # stop containers, keep state volume
docker compose down -v       # stop containers and delete state volume
```

## Locked state fixture

`fixtures/apps/api/` has a fake `.state/apps-api.tfstate.lock.info` for
testing the `U` (force-unlock) keybinding. The lock ID is
`deadbeef-1234-5678-abcd-000000000000` held by `ian@devbox`.
