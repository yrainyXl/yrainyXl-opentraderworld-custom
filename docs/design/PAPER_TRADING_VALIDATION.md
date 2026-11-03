# Paper Trading Agent implementation and validation

Date: 2026-11-01. Branch: `feature/paper-trading-agent`.

Draft PR: https://github.com/yrainyXl/yrainyXl-opentraderworld-custom/pull/1

## Scope and safety

The backend implements isolated strategies, immutable run snapshots, server-priced simulated fills, and order audits. Portfolio Tracker operations remain the sole fill ledger. Strategy setup/control requires a browser session; MCP only exposes the allowlisted paper read/asset/run/order operations. The optional `mcp:paper-trade` OAuth scope grants no generic Portfolio writes. Existing grants do not gain trading access.

Execution locks the strategy and reads/writes through one database transaction connection. Cash, positions, risk, FX, quote freshness, fees and slippage are checked before the Portfolio fill and audit commit together. Tests exercise one-connection pools, simultaneous orders competing for cash, and a deliberately failed audit insert to verify rollback. Paper assets cannot be redenominated after creation. Exact retries survive ended strategies and missing live quotes; changed payloads conflict. Research excludes paper holdings, and ordinary Portfolio mutations/imports reject paper ledgers.

No deployment, merge, real brokerage routing, production migration, or production Portfolio operation was performed. CI uses a disposable PostgreSQL 17 service without production secrets. Local Rust/Docker were unavailable and direct cloning failed; source inspection and commits used the GitHub connector, with executable verification in GitHub Actions using the repository's pinned Rust environment.

## Validation

The final workflow checks source commit `22d008dfe22490169f3842ca989fea1f5bc56278`:
https://github.com/yrainyXl/yrainyXl-opentraderworld-custom/actions/runs/41285360080

| Check | Result |
| --- | --- |
| Repository `scripts/cargo-check.sh` | Passed |
| rustfmt and clean diff | Passed, no diff |
| Store library | 53 passed |
| Shared Market Quotes database | 6 passed |
| OAuth database | 5 passed |
| Paper Trading database | 15 passed |
| Price Observations database | 7 passed |
| Core binary | 461 passed, 4 explicitly ignored |

The final workflow completed successfully: all 547 tests passed with no failures. The four existing ignored Core tests are `align::tests::bench_merge_cost`, `backtest::custom::parity::dump_series`, `backtest::xcheck::dump`, and `video_embed::live_tests::oembed_answers_with_a_title_and_an_allowlisted_poster` (benchmark/fixture generation/external-video checks). No new paper tests are ignored. This report pins the executable source commit; the subsequent documentation commit does not change the tested source. There are no outstanding implementation blockers.

Commands run by `.github/workflows/paper-trading.yml`:

```sh
scripts/cargo-check.sh
rustfmt --edition 2021 --config skip_children=true \
  core/otw-store/src/paper_trading.rs \
  core/otw-store/src/portfolios.rs \
  core/otw-store/src/journal_fx.rs \
  core/otw-store/tests/paper_trading.rs \
  core/otw-core/src/paper_trading_api.rs
git diff --exit-code
cargo test -p otw-store --lib --test paper_trading --test market_quotes --test oauth --test price_observations
cargo test -p otw-core --bin otw-core
```

The paper integration suite requires DATABASE_URL and never silently skips. It covers isolation, idempotency, immutable run snapshots, nanosecond timestamp retries, ownership and run-direction checks, cash/order/position limits, overselling, strategy lifecycle, stale/future/missing quotes, immutable asset metadata, conservative China T+1 and UTC+8 dates, USD/CNY conversion, fees/slippage, cross-asset risk, concurrency, transaction rollback, Research exclusion and OAuth opt-in/refresh. Core tests cover rejected client prices/fees/account snapshots and MCP catalog/scope boundaries.

## Exact PR file inventory

The branch already contained an unverified first implementation at `eb55bc2bbf60d644d35cb40bd903bfc6db3fbdce`. This continuation reviewed and completed it. The full PR, including that existing implementation, changes:

- `.github/workflows/paper-trading.yml`
- `CHANGELOG.md`
- `CUSTOM_ARCHITECTURE.md`
- `CUSTOM_CHANGES.md`
- `core/otw-core/src/investment_cases_api.rs`
- `core/otw-core/src/main.rs`
- `core/otw-core/src/mcp/catalog.rs`
- `core/otw-core/src/mcp/mod.rs`
- `core/otw-core/src/oauth.rs`
- `core/otw-core/src/paper_trading_api.rs`
- `core/otw-core/src/portfolios_api.rs`
- `core/otw-core/src/portfolios_import_api.rs`
- `core/otw-store/migrations/9000000110_paper_trading.sql`
- `core/otw-store/src/investment_cases.rs`
- `core/otw-store/src/journal_fx.rs`
- `core/otw-store/src/lib.rs`
- `core/otw-store/src/oauth.rs`
- `core/otw-store/src/paper_trading.rs`
- `core/otw-store/src/portfolios.rs`
- `core/otw-store/tests/paper_trading.rs`
- `docs/design/PAPER_TRADING_AGENT.md`
- `docs/design/PAPER_TRADING_VALIDATION.md`

## Commits from this continuation

- `18f39550add5a7414484797a2b91680975b3cbba` — disposable PostgreSQL CI and the repository check toolchain.
- `692c4308bbbb25e24c655ff14c9e9214d34c8f6a` — single-connection transactional fills, immutable assets, ledger integration tests, SQLx dynamic-query fixes.
- `d9300c8872df077ebcc5b0fe9b49411b30b410de` — API/MCP boundary tests and OAuth consent compilation fix.
- `43ee41c7d137fc5b7f9bd6bc07a7c300468d49f8` — coherent account contexts, settlement dates, risk/fee/retry invariants, concurrency/rollback/Research tests and docs.
- `e511478b5b9576d6ede65c95063576b2d26e230d` — supported FX fixture source.
- `80194d941338ba268636d5844852aa1a17bee6c5` — timestamp/quantity/equity boundary fixes, regression assertions, formatting and Core test type fix.
- `22d008dfe22490169f3842ca989fea1f5bc56278` — final rustfmt output and LF normalization.

## Remaining product boundaries

This is an immediate-fill paper backend. It does not model an exchange order book, partial fills, market sessions, holidays, board lots, or corporate actions. Until authoritative settlement metadata exists, every China-provider asset conservatively uses T+1. There is no strategy comparison UI, scheduled ChatGPT run configured by this PR, or automated performance report. Multi-user Portfolio ownership remains the repository's existing architectural limitation. No live provider/ChatGPT scheduled-task smoke test or production migration was attempted because deployment is outside this task.

Before enabling in production, separately review and authorize deployment/migration and OAuth re-consent. This report is evidence for review, not authorization to deploy. See the design for API payloads and C-018 for migration/rollback considerations.
