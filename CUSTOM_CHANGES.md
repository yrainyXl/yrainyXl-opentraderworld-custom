# CUSTOM_CHANGES.md — OpenTraderWorld 私有定制变更记录

> **定位**：本文件记录本私有分支相对 OTW upstream 的全部重要定制，是「Upstream 可升级性」的核心配套文档（见 `CUSTOM_ARCHITECTURE.md` §9.3）。
>
> **使用规则**：
> - 每次完成一项定制，必须在本文件追加一条变更记录（原因 / 文件 / migration / 冲突点 / 验证 / 回滚）。
> - 新建 migration 之前，必须先查本文件「§3 Migration 编号登记」，禁止与 upstream 或已有 custom 编号冲突。
> - 升级 upstream 前后，按「§5 Upstream 升级检查清单」逐项核对。
> - 本文件只记录**定制事实**，不写临时 bug 修复与一次性调试日志。

---

## 1. 定制总览（已完成 · Phase 0 稳定区）

| # | 定制项 | 状态 |
|---|---|---|
| 1 | China Market sidecar（`cn-market`，腾讯主 + 新浪 fallback） | ✅ 已完成 / 稳定 |
| 2 | Watchlist 支持 A 股（provider `china`，搜索 / 添加 / 刷新 / 行情展示） | ✅ 已完成 / 稳定 |
| 3 | Portfolio Tracker 支持 A 股（CNY 资产、买入卖出、实时定价、盈亏） | ✅ 已完成 / 稳定 |
| 4 | CNY 币种支持（A 股资产以 CNY 记账，按 Journal FX 汇率折算 USD） | ✅ 已完成 / 稳定 |
| 5 | 涨红跌绿（A 股/CNY 行按国内习惯：涨红跌绿；其他市场保持涨绿跌红） | ✅ 已完成 / 稳定 |
| 6 | Custom migrations（`0100`、`0101`，编号段 0100+ 为私有保留段） | ✅ 已完成 / 稳定 |
| 7 | Docker / Rust 构建适配（国内镜像源 + BuildKit Cargo 缓存） | ✅ 已完成 / 稳定 |

已人工验证的链路（以 `601118 海南橡胶` 为例）：Watchlist 搜索 → 添加 → 刷新价格；Portfolio 添加 CNY 资产 → 记录买入/卖出 → 刷新实时价 → 成本/持仓/市值/盈亏计算正确；行情/盈亏按涨红跌绿显示。

**这是后续所有开发的基础，不要推翻重做。**

---

## 2. 变更记录

### C-001 cn-market（China Market sidecar）

- **修改原因**：为 OTW 提供 A 股行情（腾讯为主源，新浪为 fallback），隔离国内数据源细节，避免散落进 OTW Rust 各模块。
- **修改文件**：
  - `/opt/apps/cn-market/app.py`（FastAPI：`search_instruments` / `get_spot` / `get_bars`；腾讯主源 + 新浪 fallback）
  - `/opt/apps/cn-market/Dockerfile`、`requirements.txt`、`AGENTS.md`
  - `deploy/docker-compose.yml`：新增 `cn-market` 服务（`image: otw-cn-market:latest`，build context `../../cn-market`，**无端口映射**，仅 Docker 内部网络可达）；Core 环境变量 `OTW_CN_MARKET_URL=http://cn-market:18080`
- **数据库 migration**：无（独立服务，不触碰 OTW 数据库）。
- **API 契约**：Instrument / Quote / Bars 契约见 `CUSTOM_ARCHITECTURE.md` §7.3，需保持向后兼容。
- **Upstream 潜在冲突点**：低。新增独立服务与独立 env 变量；`OTW_CN_MARKET_URL` 为私有变量名，upstream 无此配置。
- **如何验证**：`docker compose up -d cn-market` 后，Watchlist 搜索 `601118`、添加、刷新；`curl $OTW_CN_MARKET_URL/...` 检查 quote 返回 `source: tencent`，腾讯失败时 `fallback: true`（新浪）。
- **如何回滚**：从 `deploy/docker-compose.yml` 移除 `cn-market` 服务与 `OTW_CN_MARKET_URL`，重建 Core；A 股行情入口随之不可用，其他市场不受影响。

### C-002 Watchlist 支持 A 股

- **修改原因**：Watchlist 可搜索、添加、刷新 A 股，并展示价格、24h/3d/7d/30d 变化、30 天趋势、CNY 币种、涨红跌绿。
- **修改文件**（主要）：
  - Rust：`core/otw-core/src/watchlists/quotes.rs`、`core/otw-core/src/watchlists_api.rs`、`core/otw-core/src/histdata/china.rs`、`core/otw-core/src/histdata/mod.rs`
  - 前端：`frontend/src/lib/modules/watchlists/WatchlistDetail.svelte`、`frontend/src/lib/modules/watchlists/api.js`、`frontend/src/lib/modules/watchlists/Sparkline.svelte`、`frontend/src/lib/modules/dashboard/widgets/WatchlistWidget.svelte`
- **数据库 migration**：`0100_china_provider.sql` —— `watchlist_items.provider` CHECK 增加 `'china'`；种子插入 `China Market` 连接器（`histdata_connectors` + `connector_modules` 全模块授权，`ON CONFLICT DO NOTHING`）。
- **Upstream 潜在冲突点**：
  - `watchlist_items_provider_check` 约束被重建（DROP + ADD），若 upstream 新增其他 provider 值，合并时需把 upstream 的值一并保留。
  - 种子连接器「China Market」与 upstream 种子数据模式（0047 系列）可能叠加，注意 `ON CONFLICT` 语义。
- **如何验证**：Watchlist 搜索/添加/刷新 `601118 海南橡胶`；检查价格与涨跌颜色（红涨绿跌）、CNY 币种展示、Sparkline 趋势。
- **如何回滚**：撤销 `0100`（恢复 provider CHECK 到 upstream 原值，删除种子连接器），并还原上述 Rust/前端文件。

### C-003 Portfolio Tracker 支持 A 股 + CNY + 涨红跌绿

- **修改原因**：Portfolio 可搜索 A 股、添加 CNY 资产、记录买入/卖出 operation、刷新实时价格、计算成本/持仓/市值/已实现与未实现盈亏，并按 A 股习惯显示颜色。
- **修改文件**（主要）：
  - Rust：`core/otw-core/src/portfolios/prices.rs`（china 定价 + CNY→USD 按 Journal FX 汇率折算）、`core/otw-core/src/portfolios_api.rs`
  - 前端：`frontend/src/lib/modules/portfolios/AddAssetModal.svelte`、`frontend/src/lib/modules/portfolios/PortfolioDetail.svelte`、`frontend/src/lib/modules/portfolios/ReconcileModal.svelte`、`frontend/src/lib/modules/portfolios/api.js`、`frontend/src/routes/portfolios/+page.svelte`
- **数据库 migration**：`0101_portfolio_china.sql` —— `portfolio_assets.provider` CHECK 增加 `'china'`；`spot_provider` 允许 `'china'`。
- **Upstream 潜在冲突点**：
  - `portfolio_assets_provider_check` / `portfolio_assets_spot_provider_check` 被重建，upstream 若扩展 provider/spot_provider 枚举需合并保留。
  - CNY 折算逻辑基于 Journal FX 汇率表，若 upstream 汇率表结构变化需同步调整 `portfolios/prices.rs`。
- **如何验证**：添加 A 股资产（CNY）→ 记录买入 → 刷新价格 → 校验成本/持仓/市值/浮盈亏；跨币种组合价值（CNY→USD）正确；颜色按涨红跌绿。
- **如何回滚**：撤销 `0101` 并还原 `portfolios/prices.rs`、`portfolios_api.rs` 与前端文件。

### C-004 涨红跌绿（A 股配色约定）

- **修改原因**：国内习惯「涨红跌绿」；upstream 默认「涨绿跌红」。仅 A 股/CNY 行启用国内约定，其他市场保持原样，0 涨跌保持中性。
- **修改文件**：前端共享格式与各展示组件（`WatchlistDetail.svelte`、`Sparkline.svelte`、`WatchlistWidget.svelte`、`PortfolioDetail.svelte`、`portfolios/+page.svelte` 等，随 C-002/C-003 一并修改）。
- **数据库 migration**：无。
- **Upstream 潜在冲突点**：中。前端共享格式函数若被 upstream 重构（§9.4 第 8 项检查点），配色判定逻辑需重新合入。
- **如何验证**：A 股上涨行显示红色、下跌行绿色；美股/加密行仍为绿涨红跌。
- **如何回滚**：还原前端格式函数中的 CN 分支判定。

### C-006 v0.0.9 升级（2026-09-01）

- **修改原因**：从官方基线 v0.0.8 升级到 v0.0.9（Automator 模块、Backtest DCA/Optimizer、Editor 视频块、histdata rate-limit 等）。
- **升级路径**：`vendor/upstream` 线性化 —— synthetic v0.0.9（`85f25a7350ca48bb4dfcf286b281012ff29babd8`，tree 与官方 v0.0.9 一致，parent = v0.0.8）；`custom/main` 三方 merge（merge base = v0.0.8）。merge commit：`<见 §8 执行记录>`。
- **冲突与修复**：
  - 冲突仅 `frontend/src/lib/i18n/en.json`、`zh.json`（保留 upstream Yahoo 新文案 + custom China 文案）。
  - `core/otw-core/src/histdata/china.rs`：`Capability` 补 `min_interval_ms: 1_000`（upstream 新增字段，编译必修）。
  - `CHANGELOG.md`：custom 条目移回独立 `[Unreleased]`（自动合并曾并入 `[0.0.9]`）。
- **数据库 migration**：新增 upstream `0092–0097`（全部 additive + `IF NOT EXISTS`，无数据迁移）；custom `9000000100/101` 原值不变（checksum/installed_on 未动）。
- **Upstream 潜在冲突点**：`histdata/mod.rs`（Capability/SUPPORTED_TIMEFRAMES）、`watchlists_api.rs`（ImportBody JsonSchema，与 custom china 逻辑无交集）。
- **如何验证**：cargo check + tests（otw-store 34、otw-core 444 passed）+ frontend build + 冒烟（login/watchlist/portfolio/editor/automator 全链路 200）。
- **如何回滚**：见 §8.2（DELETE 92–97 rows + 恢复 rollback image；旧 binary + 已应用 92–97 的 DB 会 VersionMissing 拒绝启动，必须同时回滚）。

### C-005 Docker / Rust 构建适配（国内网络 + BuildKit 缓存）

- **修改原因**：`gcr.io`、`static.rust-lang.org`、`crates.io` 在服务器网络不稳定；同时要求后续 Rust 小改显著复用编译缓存，禁止反复冷启动 release build。
- **修改文件**：`core/Dockerfile`（私有重写）。
  - Rust 工具链/依赖源：`ENV RUSTUP_DIST_SERVER=https://rsproxy.cn`、`RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup`；`$CARGO_HOME/config.toml` 走 `sparse+https://rsproxy.cn/index/`，`git-fetch-with-cli = true`。
  - BuildKit 缓存：`RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git --mount=type=cache,target=/build/target cargo build --release --bin otw-core`，并在**同一个 RUN** 内把 binary 复制到普通目录 `/out/otw-core`，供 runtime stage COPY。
  - Runtime 基础镜像：`gcr.m.daocloud.io/distroless/cc-debian12`（DaoCloud 镜像替代 gcr.io）；非 root `USER 65532`；`/data/uploads` 预建并 chown 65532。
- **数据库 migration**：无。
- **Upstream 潜在冲突点**：高。upstream 若修改 Dockerfile（基础镜像、构建方式、运行时配置），需手动合入本文件的镜像源与缓存设计。
- **如何验证**：第一次冷编译可慢；随后修改少量 Rust 文件重新 build，应显著复用 registry/git/target 缓存（观察 BuildKit 缓存命中日志），且不触发网络重下载。
- **如何回滚**：恢复 upstream Dockerfile（注意同时失去国内镜像与缓存加速）。

---

## 3. Migration 编号登记

**编号策略（长期约定，2026-09-01 起）**：

- `0001`–`999999999`：upstream 编号段，**禁止占用**（官方 master 已到 0097，会继续增长）。
- `9000000000`+：私有定制保留 namespace，与 upstream 永不撞号。
- 每次新建前：① 查 upstream 最新 migration 编号；② 查本表已占用编号；③ 再确认本地 `core/otw-store/migrations/` 实际文件。

**已占用（custom）**：

| 编号 | 文件 | 内容 | 状态 |
|---|---|---|---|
| 9000000100 | `9000000100_china_provider.sql` | Watchlist：`provider='china'` + 种子 China Market 连接器 | ✅ 已应用 |
| 9000000101 | `9000000101_portfolio_china.sql` | Portfolio：`provider='china'` + `spot_provider='china'` | ✅ 已应用 |

**一次性 namespace cutover（2026-09-01）**：custom migration 最初以 `0100`/`0101` 命名并已应用到数据库（`_sqlx_migrations` version 100/101，checksum `26c7a93a…`/`d6df54fb…`）。为规避与未来 upstream 编号（官方已到 0097，即将触及 0100）冲突，执行了 history cutover：

1. 文件 rename：`0100_*` → `9000000100_*`、`0101_*` → `9000000101_*`（SQL 内容不变，SHA-384 不变）；
2. 数据库：`UPDATE _sqlx_migrations SET version = 9000000100 WHERE version = 100`（101 同理），description / checksum / installed_on / success 全部保持原值；
3. 新旧 binary 与 DB 版本必须同步切换（旧 binary + 新 DB、新 binary + 旧 DB 均不兼容，见「回滚红线」）。

**基线**：接入定制时 upstream 最新 migration 为 `0091_findb_fav_key.sql`；官方 master（v0.0.9）已到 `0097_automator_draft.sql`（2026-09-01 确认）。

**其他保障**：`core/otw-store/build.rs` 已保留 `println!("cargo:rerun-if-changed=migrations")`，migration 文件变化会自动触发 `sqlx::migrate!` 重编译；禁止通过修改 `lib.rs` 注释强制重建。

---

## 4. 构建 / 部署要点

- 开发期 Rust 检查统一走 `scripts/cargo-check.sh`（toolchain 固定 `rust:1.98.0-slim`，与 `core/Dockerfile`、`core/rust-toolchain.toml` 一致；复用持久缓存卷 `otw-check-cargo` / `otw-check-target`）→ 必要 `cargo test -p otw-core` → 全部通过后才做**一次** Docker release build（禁止每改几行就 release build）。
- 部署默认**不做破坏性操作**：禁止 `docker compose down -v`、删 PostgreSQL volume、清用户数据、重置 connectors/settings；只重建必要服务并保留数据卷。
- `cn-market` 只监听 Docker 内部网络（无端口映射），对宿主机不可见。

---

## 5. Upstream 升级检查清单

升级 OTW 前后必须逐项检查（对应 `CUSTOM_ARCHITECTURE.md` §9.4）：

1. migrations 是否撞号（对照 §3 编号登记）。
2. Connector / Market API 是否变化（`OTW_CN_MARKET_URL` 消费方、`cn-market` 契约）。
3. Watchlist API 是否变化（`watchlists_api.rs`、前端 `watchlists/api.js`）。
4. Portfolio API 是否变化（`portfolios_api.rs`、前端 `portfolios/api.js`）。
5. Editor document schema 是否变化。
6. MCP catalog 是否变化。
7. Dockerfile 是否变化（合并 C-005 的镜像源/缓存设计）。
8. 前端共享格式函数是否变化（涨红跌绿判定、币种/价格格式化）。

---

## 6. 文档协作基线（2026-09-01 建立）

- `AGENTS.md`（仓库根目录）：所有开发 session 的 standing orders；要求任何架构/功能开发前先读 `CUSTOM_ARCHITECTURE.md`。
- `CUSTOM_ARCHITECTURE.md`（仓库根目录）：**唯一权威**的长期架构 Source of Truth，完整内容只保存在本文件；架构性决策只修改本文件。
- `docs/design/OOTW_LONG_TERM_ARCHITECTURE.md`：仅保留指向 `../../CUSTOM_ARCHITECTURE.md` 的简短 pointer（7 行），不保存完整副本；其他文件只能引用，不得复制完整架构内容。
- `/opt/apps/cn-market/AGENTS.md`：sidecar 自己的开发纪律，并指向 `CUSTOM_ARCHITECTURE.md`。

---

## 7. Git 基线（2026-09-01 建立）

- **仓库已恢复官方 Git 历史**：`git init` + fetch 官方 `G-OTW/OpenTraderWorld`（全部 tags v0.0.1–v0.0.9 + `upstream/master`）。
- **Base commit**：`v0.0.8` = `9c4988137df5e191c163479f29daa5ca17c9d11c`（Release v0.0.8，2026-08-16）。
- **Checkpoint commit**：`custom/main` = `c4602b449610c61d0f1e18ff1891d6ec77ba9354`（custom: server checkpoint 2026-09-01，父 = v0.0.8）。
- **分支模型**：`custom/main`（定制主线）+ `upstream/master`（remote-tracking，官方 snapshot 来源）。未创建本地 `upstream/master` 分支。
- **官方 Release 是非线性 root snapshot**（每个 tag 为独立无父提交），升级纪律见 `AGENTS.md`「Upstream Git discipline」与 `CUSTOM_ARCHITECTURE.md` §9.5：
  - 禁止 `git merge upstream/master` / cherry-pick 官方 release root；
  - 升级走本地 linearized `vendor/upstream`（synthetic commit：tree = 新官方 release，parent = 上一版 vendor snapshot），再 merge 进 `custom/main`。
- **仓库外备份**：`/opt/apps/backups/otw-tree-pre-git-20260901-124803.tgz`（git 恢复前的工作树备份）。

---

## 8. v0.0.9 升级与正确回滚（2026-09-01）

### 8.1 SQLx 通用规则（重要）

SQLx 0.9 启动校验（`sqlx-core` migrator.rs `validate_applied_migrations`）：**遍历 `_sqlx_migrations` 中全部已应用行，凡 version 不在当前 binary 内嵌 migration 集合中，即返回 `MigrateError::VersionMissing(version)` 并拒绝启动**（除非显式 `ignore_missing`，本项目不启用）。

推论：

- 「旧 binary + 已应用新 migration 的 DB」**不能直接启动**（VersionMissing）。
- 「新 binary + 旧 DB」会按序应用缺失 migration，是正常升级路径。
- 因此任何升级的回滚必须**同时**回滚 binary 与 `_sqlx_migrations` 中本次新增的 rows（schema object 本身可保留，见 8.2）。

### 8.2 v0.0.9 升级回滚（正确流程）

适用场景：新 Core 已启动并自动应用了 0092–0097，需要回退到 v0.0.8（含 9000000100/101 的 cutover binary）。

```text
1. docker compose stop core
2. 事务中删除本次新增的 migration rows：
   BEGIN;
   DELETE FROM _sqlx_migrations WHERE version IN (92,93,94,95,96,97);
   COMMIT;
   -- 校验：SELECT version FROM _sqlx_migrations WHERE version BETWEEN 92 AND 97; → 0 rows
3. 不需要删除 92~97 新增的 tables/columns/indexes：
   0092–0097 均为 additive + IF NOT EXISTS，
   schema object 保留无害（旧 binary 不引用它们）。
4. 恢复升级前 rollback Core image（opentraderworld/otw-core:rollback-pre-v0.0.9）。
5. 启动旧 Core（此时 DB 中已无 92~97 rows，不会 VersionMissing）。
6. /api/health 验证。
7. 查询 _sqlx_migrations：92~97 = 0 rows；9000000100/101 保持存在。
```

重新升级到 v0.0.9 时，新 Core 会重新执行 0092–0097；`IF NOT EXISTS` 保证已有 schema object 不冲突，并重新写入 migration metadata。

### 8.3 升级路径（vendor/upstream 线性化）

- 官方 release 是独立 root snapshot（禁止直接 merge/cherry-pick），升级走 `vendor/upstream`：
  - `vendor/upstream` = v0.0.8 → synthetic v0.0.9（tree 与官方 v0.0.9 一致，parent = v0.0.8）。
  - `custom/main` merge `vendor/upstream`（三方 merge base = v0.0.8）。
- v0.0.9 升级已知冲突/修复点（dry-run 验证）：
  - 冲突仅 `frontend/src/lib/i18n/en.json`、`zh.json`（保留 upstream Yahoo 新文案 + custom China 文案）。
  - `histdata/china.rs` 需补 `min_interval_ms: 1_000`（upstream 给 `Capability` 新增字段）。
  - `CHANGELOG.md` 需把 custom 条目移回独立 `[Unreleased]`（自动合并会并入 `[0.0.9]`）。

### 8.4 v0.0.9 升级执行记录（2026-09-01）

- **vendor commit**：`85f25a7350ca48bb4dfcf286b281012ff29babd8`（synthetic v0.0.9，tree 与官方 `1ab52ad` 一致）。
- **merge commit**：`5dbfc93f7f6c99d9c40a56dc48f3d0057f92dc2e`（custom/main merge vendor/upstream；冲突 2 个 i18n 文件已按 §8.3 解决）。
- **数据库**：新 Core 启动时自动应用 `0092–0097`（installed_on `2026-09-01 14:12:46 UTC`，success=t）；`9000000100/101` checksum/installed_on 原值未变。
- **镜像**：新 core `a5df3007…`（v0.0.9）；新 frontend `23fcd0d6…`；rollback tags `otw-core:rollback-pre-v0.0.9` / `otw-frontend:rollback-pre-v0.0.9`。
- **备份**：`/opt/apps/backups/otw-db-pre-v0.0.9-20260901-220807.sql`（136M）。
- **验证**：cargo check ✓、otw-store 34 tests ✓、otw-core 444 passed ✓、frontend build ✓、冒烟全链路 200（login/watchlist 601118+refresh/portfolio 601118 avg_cost 6.49 unrealized +60.0/editor create-patch-read-delete/automator workflows+schedules+catalog）。
