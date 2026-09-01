# OpenTraderWorld Custom Development Instructions

This repository is a privately customized OpenTraderWorld instance.

## Mandatory architecture reference

Before implementing any feature, architecture change, database migration,
market-data integration, Portfolio/Journal/Editor integration, Agent/MCP work,
or deployment change, MUST read:

./CUSTOM_ARCHITECTURE.md

Treat that document as the long-term architecture Source of Truth.

Do not implement a request in a way that conflicts with it merely because
a shortcut is easier.

## Core invariants

1. Same fact must have one authoritative source.
   - Market data → Market/Connector layer
   - Trades/positions → Portfolio Tracker
   - Investment thesis → Investment Case
   - Decisions → Decision Records
   - Statistical analysis → Trading Journal

2. Do not duplicate Portfolio trade facts into Journal or Editor.

3. China-market provider details belong in:
   /opt/apps/cn-market

   Do not spread Tencent/Sina-specific parsing across OTW modules.

4. Gradually converge Watchlist, Portfolio, Research and Historical Data
   onto a shared Market / Instrument abstraction.

5. Do not hard-code individual symbols such as 601118.

6. Preserve upstream upgradeability.
   Keep custom business logic modular and record important custom changes.

## Rust development rules

Rust checks go through `scripts/cargo-check.sh` — it pins the toolchain
(rust:1.98.0-slim, in sync with core/Dockerfile and core/rust-toolchain.toml)
and reuses the persistent cache volumes `otw-check-cargo` / `otw-check-target`,
so every session gets a warm cache instead of a cold compile.

Do NOT repeatedly perform Docker release builds while developing.

Check scope by change type:

- Markdown/docs only → no Rust check
- frontend only → frontend check only (no Rust check)
- cn-market / Python only → Python check only
- migration rename or content-only change → no standalone cargo check;
  the final Core build verifies the embedded migrations
- Rust source changed → scripts/cargo-check.sh, then targeted tests
- production deployment → one `docker compose build core` at the end

Development sequence for Rust changes:

1. inspect source and git diff
2. modify code
3. scripts/cargo-check.sh
4. targeted tests
5. only when deploying: one Docker release build

Docker builds must preserve Cargo registry/git/target BuildKit caches.
The Docker core builder toolchain must stay in sync with
scripts/cargo-check.sh and core/rust-toolchain.toml.

## SQLx migrations

Before creating a migration:

1. inspect upstream migration numbers
2. inspect local custom migrations
3. avoid version collisions
4. preserve build.rs rerun-if-changed=migrations behavior

Never modify lib.rs comments merely to force sqlx migrations to rebuild.

## Data safety

Never, unless explicitly requested:

- docker compose down -v
- delete PostgreSQL volumes
- wipe user data
- reset existing connectors/settings
- modify unrelated applications under /opt/apps
- modify Rhythm
- expose new public ports

## Change reporting

After a substantial task, report:

- files changed
- schema/migrations
- API changes
- tests performed
- deployment steps
- rollback procedure
- likely upstream conflicts

If an architectural decision changes the long-term design,
update CUSTOM_ARCHITECTURE.md instead of silently deviating from it.

## Upstream Git discipline

Official OpenTraderWorld release tags (v0.0.1 ... v0.0.9) are independent
root snapshot commits, NOT linear Git history.

- NEVER run `git merge upstream/master` or cherry-pick an official release
  root directly onto custom/main.
- Treat upstream tags / upstream/master ONLY as the source of official
  snapshots.
- Upgrading must go through a local linearized `vendor/upstream` branch:
  - create ONE synthetic commit whose tree equals the new official release
    and whose parent is the previous vendor snapshot;
  - then merge `vendor/upstream` into `custom/main` so a normal 3-way merge
    base exists.
- Do not skip the vendor step by merging release roots directly.
