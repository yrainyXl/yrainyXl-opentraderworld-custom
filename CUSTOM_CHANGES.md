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

### C-007 Investment Case MVP（Phase 1，2026-09-01）

- **修改原因**：从 Portfolio 资产行进入按 instrument 的研究页面；决策带冻结快照；研究正文复用 Editor。
- **修改文件**：
  - 后端：`migrations/9000000102_investment_cases.sql`、`core/otw-store/src/investment_cases.rs`（新）、`documents.rs`（`create_with` tx-compatible 重构）、`portfolios.rs`（`position_usd`/`asset_view` → `pub(crate)`，仅可见性）、`lib.rs`、`core/otw-core/src/investment_cases_api.rs`（新，5 端点 + 每 handler ownership 校验）、`documents.rs`（research 文档 ownership guard + DELETE 409 + FK 23503 映射）、`main.rs`。
  - 前端：`routes/investment-cases/[id]/+page.svelte`、`lib/modules/investment-cases/{api.js,DecisionModal.svelte}`、`PortfolioDetail.svelte`（研究入口）、i18n en/zh。
  - 文档：`docs/design/PHASE1_INVESTMENT_CASE.md`（设计 v2）、`CHANGELOG.md`。
- **数据库 migration**：`9000000102_investment_cases.sql` —— `investment_cases`（`user_id → users(id) ON DELETE CASCADE`，`UNIQUE(user_id, provider, provider_id)`，`document_id → documents(id) ON DELETE RESTRICT`）+ `decision_records`（冻结 snapshot，`decision_type` CHECK 6 值，`confidence smallint 1–5`）。
- **财务口径**：live/快照复用 `position_usd` USD ledger（逐笔按 op_date FX）+ 今日 FX 折回 instrument 币种 —— 与 Portfolio 页完全一致（接受用例 C 数值相等）；avg cost 为 ledger 显示值，非历史 native（既有产品语义）。
- **并发安全**：`ensure_case` 事务内 `ON CONFLICT DO NOTHING` + 竞争失败整体 rollback（document 不落库）→ 1 case + 1 document + 0 orphan（10 并发实测）。
- **Upstream 潜在冲突点**：低（全部为新增文件/新增表；仅 `documents.rs`/`portfolios.rs` 小改动，冲突面小）。
- **如何验证**：cargo check + tests（otw-store 35、otw-core 445）+ frontend build + 冒烟 A–J（见设计文档 §N）。
- **镜像**（实查 2026-09-01）：Phase 1 前（v0.0.9 升级产物）core = `a5df3007…` → 已打 `otw-core:rollback-pre-phase1`；Phase 1 初版 build = `51e44f89…`（已被替换）；**最终 running/latest = `7be5ee04…`**（含 price_at RFC3339 修复）；frontend running/latest = `e909c50d…`，rollback = `23fcd0d6…`（`otw-frontend:rollback-pre-phase1`）。已验证 running == latest 且 running != rollback。
- **如何回滚**（正确流程，schema 与 migration metadata 必须一致）：
```text
1. docker compose stop core
2. 备份 Phase 1 两表数据（pg_dump 或 CREATE TABLE ... AS SELECT）
3. 一个事务内：
   BEGIN;
   DROP TABLE decision_records;      -- 先删子表（FK → investment_cases）
   DROP TABLE investment_cases;      -- 引用 documents 的表被删除，documents 不受影响
   DELETE FROM _sqlx_migrations WHERE version = 9000000102;
   COMMIT;
   -- 不能只 DELETE rows 而保留表：否则 SQLx 认为 9000000102 未执行，
   -- 重新部署 Phase 1 时 CREATE TABLE 会与遗留表冲突。
   -- 也不要把 9000000102 的 IF NOT EXISTS 实现细节当作回滚策略。
   -- 注意：Phase 1 创建的研究 documents 会保留为普通页面（无害，可手动清理）。
4. 恢复 otw-core:rollback-pre-phase1 + otw-frontend:rollback-pre-phase1
5. 启动 → /api/health 验证
```

### C-009 Investment Case 提升为一级「研究」模块（导航修正，2026-09）

- **修改原因**：Investment Case 目前只能从 Portfolio 详情入口到达，且未注册模块 → `/investment-cases` 及详情路由在模块切换器 / 页面标题中归属错误（回退显示 Dashboard/仪表盘）。本轮把 Investment Case 提升为独立一级「研究」模块（路由不变），新增 Research 首页列表；Portfolio「研究」入口保留为上下文快捷入口。**无 schema / migration 改动**。
- **修改文件**：
  - 后端：`core/otw-store/src/data_admin.rs`（known modules 目录新增 `investment-cases`，含纯测试）、`core/otw-store/src/investment_cases.rs`（新增 `InvestmentCaseRow` + `list_cases_for_user`：cases + 最近 change record 时间）、`core/otw-core/src/investment_cases_api.rs`（`GET /api/investment-cases`）。
  - 前端：`lib/modules/registry.js`（注册一级模块 `investment-cases`（icon target，base `/investment-cases`）+ `moduleLabel()` 名称本地化助手）、`lib/modules/ModuleSwitcher.svelte` / `lib/settings/ModulesSection.svelte` / `lib/ui/GlobalSearch.svelte` / `routes/+page.svelte`（模块名改用 `moduleLabel`，支持 nameKey 本地化）、**新** `routes/investment-cases/+page.svelte`（列表页）、`routes/investment-cases/[id]/+page.svelte`（加「← 研究」归属 breadcrumb）、`lib/modules/investment-cases/api.js`（`list()`）、i18n en/zh。
  - 文档：`CHANGELOG.md`。
- **新增路由**：`/investment-cases`（列表页）；详情 `/investment-cases/{id}` 归属改为「研究」。
- **后端 list API**：新增最小只读 `GET /api/investment-cases`（返回当前用户 cases，按最近 change 时间倒序；P&L **不在**列表接口中计算）。列表页每行实时持仓/浮盈来自既有 `GET /investment-cases/{id}/live`（Portfolio SSOT 聚合唯一实现点，前端并行拉取、失败降级）。
- **模块安装语义**：前端 registry 与后端 `data_admin::MODULES`（known ids）为**两份需同步的清单**；`investment-cases` 已两侧登记。缺省（无 `installed_modules` 设置）= 全部模块已装；若线上曾执行过 detach/install 固化过该设置，需把 `investment-cases` 追加进已装集合（部署时校验）。
- **Upstream 潜在冲突点**：低。全部 additive；若 upstream 改动 registry/data_admin MODULES（新增模块或重构 nameKey 机制）需三方合入。
- **如何验证**：cargo check ✓、otw-store `data_admin::tests` ✓、frontend build ✓；部署后人工冒烟：主导航出现「研究」（zh）/Research（en）→ 列表含已有案例 → 点击进详情（导航高亮 Research，不再显示 Dashboard）→ Portfolio「研究」按钮仍可用且打开同一 case 实体 → `GET /api/investment-cases` 返回该用户 cases。
- **如何回滚**：还原代码并重建 core/frontend（无 migration，数据零改动）。

### C-008 Editor 研究文章可与投资案例解绑删除 + 「股票研究」目录（2026-09）

- **修改原因**：Editor 中删除被投资案例引用的研究文章时返回 `409 document is referenced by an investment case; delete the case first`，而产品内**不存在**删除投资案例的入口 —— 死胡同。产品语义修正：案例与文章只是**可断开的关联**（文章删除不应破坏案例的决策/投资逻辑），删除后重新进入研究会**自动新建一篇空文章**并重新关联；同时研究文章自动放入根目录「股票研究」文件夹，不再散落在树根。
- **修改文件**：
  - 后端：`core/otw-store/src/investment_cases.rs`（`InvestmentCase.document_id` → `Option`；`ensure_case` 存在但无文档时自动补建；新增 `RESEARCH_FOLDER_TITLE`、`ensure_research_document` / `ensure_research_folder` / `attach_fresh_document`；研究文档创建改到文件夹下）、`core/otw-store/src/documents.rs`（新增 `get_meta`，事务内取文档元数据）、`core/otw-core/src/documents.rs`（删除 handler 移除投资案例 409 预检，注释更新）、`core/otw-core/src/investment_cases_api.rs`（新增 `POST /api/investment-cases/{id}/research-document`）。
  - 前端：`frontend/src/lib/modules/investment-cases/api.js`（`ensureResearchDocument`）、`frontend/src/routes/investment-cases/[id]/+page.svelte`（reload 容忍 `document_id` 为 null；打开「补充研究笔记」且无文档时懒建新文章并重关联）。
  - 文档：`CHANGELOG.md`。
- **数据库 migration**：`9000000104_investment_case_document_link.sql` —— ① `investment_cases.document_id DROP NOT NULL`；② FK `investment_cases_document_id_fkey` 重建为 `ON DELETE SET NULL`（删除研究文章 → 自动解绑案例，而非阻塞）；③ 数据迁移：确保根目录「股票研究」文件夹存在（同名已存在则复用），把仍位于根目录、且被 investment_cases 引用的 page 移入该文件夹（用户已自行归档的页面不动）。
- **并发安全**：`ensure_research_document` 快路径无副作用只读；慢路径 `FOR UPDATE` 锁 case 行后重建 + 重关联，两个并发重建不会重复建页；`ensure_case` 竞争失败仍整体 rollback（不落 orphan document），返回 winner case 后走 `ensure_case_document` 保证其研究页存在。
- **Upstream 潜在冲突点**：低。全部改动位于 custom 新增模块/表（investment_cases 系列）与 custom migration 段；`documents.rs` 仅新增 `get_meta` 函数（additive）。
- **如何验证**：cargo check ✓、otw-store `investment_cases::tests` ✓、frontend build ✓（未跑浏览器冒烟，见部署后人工验证）。部署后人工冒烟：① 案例研究页在 Editor 删除 → 200，案例仍在、决策记录仍在，案例页「补充研究笔记」重开自动新建空白页；② Editor 树根出现「股票研究」文件夹且存量根级研究文章已移入（含 78c29f2d… 所属案例的页面）；③ Portfolio「研究」重进自动补建文章。
- **如何回滚**：
```text
1. docker compose stop core
2. 事务内：
   BEGIN;
   DELETE FROM _sqlx_migrations WHERE version = 9000000104;
   -- 若需把移到「股票研究」的页面移回根目录（数据回滚）：
   UPDATE documents d SET parent_id = NULL
     FROM investment_cases ic
     WHERE ic.document_id = d.id AND d.parent_id IS NOT NULL
       AND d.parent_id = (SELECT id FROM documents
                          WHERE kind='folder' AND title='股票研究' AND parent_id IS NULL LIMIT 1);
   COMMIT;
   -- 可保留/手动删除「股票研究」空文件夹
3. 恢复回滚前 core/frontend images（若已有）并启动
   -- 注意：新 binary + 已删 9000000104 row 的 DB 会重新执行该 migration（幂等：FK SET NULL /
   -- NOT NULL 改动重放安全；文件夹 IF NOT EXISTS 语义重复插入需手检，推荐按 §8.1 规则同时回滚 binary）
```

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

### C-010 Editor 通用「文档大纲 / Document Outline」（2026-09）

- **修改原因**：长文档需要语义 Heading 自动生成目录——点击跳转章节、正文滚动时高亮当前章节。作为 Editor 层**通用能力**，Editor 普通页面与 Investment Case「补充研究笔记」共用同一份实现；不写死任何业务页面。
- **核心设计**：Rich Text Document = SSOT；`outline.js` 单次遍历 ProseMirror JSON 的语义 `heading` 节点（H1–H3，StarterKit `heading` 节点，非字号/加粗猜测）派生出 `{level, text}` 列表。Outline 永远是派生数据：
  - 不修改 document 存储格式、不加 migration、不加独立 TOC 表/列、不批量写 UUID/anchor id；
  - heading 文本/增删/level 变更都经 TipTap `update`（docChanged 才触发，selection-only 不重扫）自动重算，rAF 节流避免每键全量扫描。
- **修改文件**（全部 frontend，无 Rust）：
  - 新增 `frontend/src/lib/modules/editor/outline.js`（纯派生，无 UI/存储依赖，未来可复用于 Markdown Export / AI Context）
  - 新增 `frontend/src/lib/modules/editor/DocumentOutline.svelte`（通用目录组件：button 语义、level 缩进、单行 ellipsis、active 轻量高亮、自带轻滚动、键盘可达）
  - `frontend/src/lib/modules/editor/Editor.svelte`：挂载右侧 sticky 大纲轨（`.doc-grid` 双列，约 236px）+ 窄屏折叠为「文档大纲 ▾」；点击跳转只滚动 Editor 自身 scroll container（等同 scrollIntoView，不改 URL）；scroll-spy 按当前 scroll position 决定 active（不假设 observer root = window，跟随 Editor 实际滚动容器）
  - `frontend/src/routes/investment-cases/[id]/+page.svelte`：research notes 的 `.editor-wrap` 由 `min-height` 改为有界 `height: clamp(420px, 60vh, 760px)`，使 Editor 内部滚动容器成为 notes 唯一正文滚动区（折叠时 Editor 未挂载 → 不显示 Outline；展开即自动出现；无双重正文滚动条）
  - i18n：`en/zh/de/es/fr/it/pt.json` 增加 `editor.editor.documentOutline`
- **数据库 migration**：无。
- **API 变化**：无。
- **Upstream 潜在冲突点**：低–中。`Editor.svelte` 与 `frontend/src/lib/i18n/*.json` 是 upstream 高频文件；新增文件（outline.js / DocumentOutline.svelte）无冲突面。合并时主要留意 Editor 布局结构（`.scroll/.column` 改为 `.frame-root/.doc-grid`）。
- **如何验证**：`npm run build` ✓；文档含 H1–H3 时右侧生成目录、缩进正确；点击跳转；滚动 active 切换；改名/删除/改 level 实时同步；普通粗体不出现；刷新后从文档内容重新生成；Investment Case notes 折叠无 Outline、展开正常。
- **如何回滚**：还原上述 frontend 文件（移除两个新文件与 Editor 布局改动），重建 frontend 镜像即可；无数据库影响。

### C-011 研究详情页重组：动态跟踪 + 研究逻辑（2026-09）

- **修改原因**：研究详情页此前同时存在「当前状态」卡片（估值/最大问题独立 autosave）、「记录变化」常驻表单与「决策时间线」，UI 与真实数据流（每轮判断 = 一条 Decision Record；当前判断 = 最新记录派生）不一致。本轮按数据流重排 UI：
  - 删除独立「当前状态」卡片与独立 autosave 路径；估值判断与当前最大问题移入「记录本轮判断」表单，作为**每轮判断的历史字段**存入决策记录（历史轮次不再被新内容覆盖）。
  - 「当前动作 / 信心 / 当前最大问题 / 当前关注(watch_items) / 最近更新」一律由最新一条 Decision Record 派生，顶部不再单独维护第二份当前状态。
- **数据模型**：`decision_records` 增加 `valuation_view`（枚举 check 与 case 级一致）与 `biggest_question`（text）；`investment_cases.valuation_view / current_biggest_question` 降级为**最新记录的只读投影**（`record_change` 事务内同步更新，供 Research 列表等既有消费方继续工作）。`watch_items` 复用既有列承载「接下来重点关注什么」。
- **修改文件**：
  - 新增 migration `core/otw-store/migrations/9000000105_investment_case_round_state.sql`（additive：两列 + check + 把 case 级存量 current state 一次性折叠进最新决策记录，不删旧列/旧数据）
  - `core/otw-store/src/investment_cases.rs`：`DecisionRecord`/`NewChange` 增字段、INSERT 与 case 投影 UPDATE 扩展；删除 `update_current_state`
  - `core/otw-core/src/investment_cases_api.rs`：`POST /decisions` 接受 `valuation_view`/`biggest_question`；删除 `PATCH /state` 路由与 handler
  - `frontend/src/lib/modules/investment-cases/api.js`：删除 `patchState`
  - `frontend/src/routes/investment-cases/[id]/+page.svelte`：整页重组（顶部横向摘要带 + 当前判断条 + 「动态跟踪/研究逻辑」页签；时间线 = 判断记录 + Portfolio 操作合并倒序；「记录本轮判断」弹窗表单；研究逻辑页签复用 thesis + Editor 正文）
  - i18n：en/zh 新增 `investmentCases.*` 标签（tabs/round/current/timeline/snapshot/portfolio/activity）
- **数据库 migration**：`9000000105`（已应用；随 Core 部署自动执行）。
- **API 变化**：新增决策入参字段；**删除** `PATCH /api/investment-cases/{id}/state`（唯一调用方为本页，已同步下线）。
- **Upstream 潜在冲突点**：中。`investment_cases_api.rs`/`investment_cases.rs` 与 custom 0102–0104 同属私有段（upstream 无 investment case）；前端 `[id]/+page.svelte` 与 `i18n` 为高频文件，合并时以本文件布局为准。
- **如何验证**：`scripts/cargo-check.sh` ✓；`cargo test -p otw-store investment_cases::tests`（如跑）；`npm run build` ✓；人工验收 A–L 流程（记录本轮判断 → 时间线顶部出现 + 顶部当前判断/关注即时派生；Portfolio 记交易后回到本页自动出现；刷新后由记录重建，无独立目录/状态数据）。
- **如何回滚**：还原上述 frontend/Rust 文件并删除 0105（回滚须同时回滚 binary 与 `_sqlx_migrations` 中 0105 行，见 §8 通用规则）；0105 为纯 additive，schema 对象保留亦无害。

### C-012 研究列表页：持仓分组 + 盈亏排序（2026-09，frontend-only）

- **修改原因**：研究列表此前为混合卡片且页面较窄；需按“持仓优先”组织：仅两个一级分组（持仓中 / 未持仓），用 Portfolio 持仓数量决定归属，持仓组按浮动盈亏率从高到低排序，未持仓组按最近判断时间倒序。
- **修改文件**：
  - `frontend/src/routes/investment-cases/+page.svelte`（列表页整页重写：分组/排序/三列卡片/空态/响应式）
  - i18n：en/zh 更新 `list.lede`，新增 `list.summary/groupHeld/groupHeldMeta/groupUnheld/groupUnheldMeta/noJudgement/holding/unheld/open/noRecords`
- **字段依据**：分组/盈亏用现有 `GET /live` 的 `position.quantity` / `unrealized_pnl` / `unrealized_pct`；当前判断（动作/信心/估值/最大问题/最近判断时间）用 case 行上的 `current_*` 投影与 `latest_change_at`；无判断只作卡片内弱提示（`尚未建立判断`），不产生分类。
- **数据库 migration**：无。**API 变化**：无（仍复用既有 per-case `/live` 并行抓取；列表 payload 本身不含持仓/盈亏字段——如需“零 N+1”，后续应在 Rust 列表接口内聚合，另行记录）。
- **待补充（接口数据不足）**：最新判断记录的 `watch_items` 未投影到列表接口 → 列表卡片不展示“当前关注”行（避免逐 case 拉全量 decisions 的 N+1）；需要时在 Rust 侧给列表行加 `latest_watch_items` 投影。
- **如何验证**：`npm run build` ✓；人工检查持仓/未持仓分组、盈亏排序、卡片跳转详情、超宽屏居中、手机无横向滚动、无新请求类型。
- **如何回滚**：还原列表页 + i18n 即可，无数据影响。

### C-013 Watchlist 价格观察 / Price Observation（2026-09-08）

- **修改原因**：用户在 Watchlist 高频盯盘时，希望「点击某只股票的 30 天 sparkline → 在**一个中大型 Modal** 内同时看到：① 系统 30 天行情（现有 Market bars）；② 自己主动记录过的价格观察点与备注；③ 快速记录当前价格；④ 编辑/删除自己的观察」。约束：不跳页、不做行内展开、不撑高 Watchlist 表格、不滚动位置、不把行情复制进 Watchlist DB、不动旧 `notes`（长期备注）与 Research/Investment Case。
- **概念边界**（本轮核心）：
  - **Market bars = 系统每日行情**（唯一来源：item quote JSONB `history` + 实时 quote，现有 refresh pipeline），观察 Modal 直接复用，**不新建 daily 表、不复制 bars、不每日写价**。
  - **watchlist_price_observations = 用户历史观察快照**（“这个时间点、这个价格附近我主动留下了一次观察”），只有它落库；`price_snapshot` 冻结创建当时用户看到的报价（DB 当前 quote），`recorded_at` = 服务端记录时刻，未来行情复权/修正不改变快照。
- **修改文件**：
  - **migration（新）**：`core/otw-store/migrations/9000000106_watchlist_price_observations.sql`。
  - **otw-store（新）**：`core/otw-store/src/watchlist_observations.rs`（Observation row + `list_for_item`/`get`/`add`/`update_note`/`delete`）；`lib.rs` 增加 `pub mod watchlist_observations;`。
  - **otw-core**：`core/otw-core/src/watchlists_api.rs`（新增 observation 路由与 handlers；POST 时服务端解析 price/currency——quote 缺失则先 `quotes::refresh_item` 一次再试；只读 `price_usd` 与 `currency`）。
  - **后端测试（新）**：`core/otw-store/tests/price_observations.rs`（7 个 targeted tests，见验证）。
  - **前端**：`frontend/src/lib/modules/watchlists/api.js`（observations CRUD）、`observations.js`（新：从 `quote.history` 重建 30 日序列（与 sparkline 同算法）+ market-tz 日期对齐）、`PriceTrendChart.svelte`（新：纯 SVG 折线 + 观察 marker + hover tooltip）、`PriceObservationModal.svelte`（新：Modal 本体）、`WatchlistDetail.svelte`（30 天 sparkline 变为可点击按钮，点击打开 Modal；行高/滚动位置不变）、i18n en/zh（`watchlists.obs.*`）。
- **数据模型 / migration**：`watchlist_price_observations`（`id` uuid PK；`item_id` UUID FK → `watchlist_items(id)` **ON DELETE CASCADE**（→ 删除 item/watchlist 连带删除观察）；`recorded_at` timestamptz（含时分秒，一天允许多条，无 UNIQUE(date)）；`price_snapshot` double precision > 0；`currency` text（CNY/USD，快照币种冻结）；`note` text 默认 '' 可空；`created_at`/`updated_at`）。索引 `(item_id, recorded_at DESC)`。**未**新增 daily/price 表，**未**复制 Market bars，**未**修改旧 `notes`/remark 字段，**未**迁移历史备注。
- **API**（风格与现有 Watchlist alerts 一致，均为 `/api/watchlists/...` 下 item 子资源）：
  - `GET /api/watchlists/items/{item_id}/observations` → `{ observations: [...] }`，按 `recorded_at DESC`。
  - `POST /api/watchlists/items/{item_id}/observations`，body `{ "note": "…" }`（可空）→ `{ observation }`。`price_snapshot` / `currency` 由服务端从 item 当前 quote 决定，`recorded_at` = 服务端 now；**不信任前端提交价格**。
  - `PATCH /api/watchlists/observations/{id}` body `{ "note": "…" }` → 仅允许改 note；`price_snapshot`/`recorded_at` 冻结不可改。
  - `DELETE /api/watchlists/observations/{id}` → `{ ok: true }`。
- **前端交互**：Modal size `lg`（≈860px，中大型）；图 = 复用 item `quote.history` 计算的 30 日系统收盘折线（A 股涨红跌绿，与行 sparkline 同色规则）；observation 以琥珀色 marker 叠加在同 market-date 的日线上（同一天多条聚合为带数字圆点）；hover 普通点显示“日期 + 收盘/当前”，hover marker 显示“日期 时间 + 记录价格 + note”；「+ 记录当前价格」在当前 Modal 内展开 inline editor（price/time 只读自动获取，note 可选），保存后列表即时刷新 + 图上 marker 立即出现；每条记录可 inline 编辑 note 或删除（轻量 ConfirmModal）；底部「查看每日价格」默认折叠，展开后为 30 日系统每日价格表（来自同一 bars，观察日显示 ●）。打开 Modal 时才 lazy 拉取该 item observations，bars 直接复用 Watchlist 已有 quote，不做首页批量加载、不刷新整个 Watchlist 页面。
- **Upstream 潜在冲突点**：低。全部为新增文件/新增表/新增 endpoint；改动仅叠加在既有 `watchlists_api.rs` 与 `watchlists/api.js`（新增代码块）。若 upstream 重构 Watchlist 路由前缀或 item 表结构需三方合入。
- **如何验证**：`scripts/cargo-check.sh` ✓；`cargo test -p otw-store --test price_observations`（7/7 ✓，见下）；frontend `npm run build` ✓；部署后人工冒烟（601118 海南橡胶）：点击 30 天 sparkline 打开 Modal → 看 30 天行情 + marker → 记录“继续上涨，观察能否站稳 8 元”→ 刷新后记录仍在 → 编辑 note / 删除后列表与 marker 同步消失 → 展开「查看每日价格」→ Watchlist 旧备注不受影响 → 关闭 Modal 回到原位置。
- **targeted tests（7）**：create 持久化 price_snapshot/recorded_at/note/currency；list 按 recorded_at DESC；不同 item 不串数据；note 编辑后快照冻结（即使 quote 之后变化）；delete 后不可再删；删 item 级联清观察；删 watchlist 级联清 item+观察。
- **如何回滚**：恢复 `otw-core:rollback-pre-observation` / `otw-frontend:rollback-pre-observation` image；DB 侧（如需彻底移除）：先备份 `watchlist_price_observations` 数据，再一个事务内 `DROP TABLE watchlist_price_observations; DELETE FROM _sqlx_migrations WHERE version = 9000000106;`（旧 binary 不内嵌 9000000106，多出的行会 VersionMissing，必须删 row）。观察功能只是用户自己加的记录，删除后无其他模块受影响。

### C-014 共享 Market Quote 边界 + Portfolio 表格右对齐（2026-09-09）

- **修改原因 / 真实链路**（修改前）：
  - Watchlist 当前价 = `watchlist_items.quote` JSONB（provider 原生价 + spark/history），由 Watchlist 自己的刷新（列表同步循环 / 手动刷新 / 添加标的）写入。
  - Portfolio 当前价 = `portfolio_assets.last_price_usd`（USD）+ `last_price_at`，由 Portfolio 自己的刷新（**每日** auto_refresh 任务 / 手动刷新 / 添加资产定价）写入；`asset_view` / `summary`（Portfolio 资产表与 P&L 的 **SSOT 计算**）只读这一列。
  - Research `/live` = `investment_cases::instrument_position`，**跨组合聚合时同样只读 `portfolio_assets.last_price_usd`**。
  - 因此 Watchlist 刷新后 Portfolio / Research 仍显示旧价；Research 必须先有 Portfolio 侧刷新才变 —— 两个模块各自维护一份互不相通的 quote 缓存，这就是 stale 的根本原因。存在多个 last_price/cache：`watchlist_items.quote`（JSONB，原生价）与 `portfolio_assets.last_price_usd`（USD）各一份。
- **最终 shared Market Quote 设计**：
  - **新增 `market_quotes` 表（additive migration 9000000107）**：每个稳定 instrument identity `(provider, provider_id)` 一行（`china:601118` 等），保存 **USD 当前价**（非 USD 原生报价在写入时用 journal FX 换算）+ 原生 `currency` + `quote_at`（观察时间）。这就是三个模块共享的 Current Market Quote 边界（DB 层 shared state；无 localStorage、无前端 store —— SPA 每次导航的 API 请求都经由该缓存，天然跨页面一致）。
  - **写入方（各自沿用既有 fetch，零额外请求）**：Watchlist 刷新成功即 upsert（`watchlists/quotes.rs`，USD 换算缺失则不写）；Portfolio refresh 仍写 `last_price_usd`，读方二者取新。**因此不写第二套 provider fetch。**
  - **读取方（SSOT 计算不动）**：`asset_view`/`asset_views`/`summary`（Portfolio）与 `instrument_position`（Research）改为「shared 行 vs 本地 last_price 取 `quote_at` 更新者」，继续走原 USD→组合币种换算链路；`pick_price_usd`（纯函数）实现该选择，供两处复用。交易事实（quantity/basis/realized 的 same-day 顺序、long-only 语义）完全不变。
  - **freshness gate（otw-core `market.rs`）**：进入 Portfolio list/detail、Investment Case `/live`、record decision 前对涉及 identity 调 `ensure_*` —— 缓存缺失或 `quote_at` 超过 **TTL=30s** 时刷新（同步，fetch 成功后写回再计算），fresh 则直接用（Watchlist 刚刷过就零请求）；provider 内去重、key 维度单飞（in-flight set，同 tick 三模块同 instrument 只发一次请求）、CoinGecko batch、其余 4 并发上限；单个 provider 抖动不回滚请求、保留 last known。`last_price_usd/last_price_at` **保留**作为 fallback / last-known quote（无 destructive migration）。
  - quote key：`provider + provider_id`（默认定价 identity）；有 `spot_provider` override（binance/kraken/coinbase…）的资产不在共享 cache 覆盖范围，继续由 Portfolio 刷新持久化自己的 last price（保持原语义）。
- **修改文件**：
  - **migration（新）**：`core/otw-store/migrations/9000000107_market_quotes.sql`。
  - **otw-store**：`src/market_quotes.rs`（新：`upsert`/`get`/`get_many`/`remove`/`age_secs`）；`src/lib.rs`；`src/portfolios.rs`（`asset_default_key`/`pick_price_usd` + `asset_view`/`asset_views` 走 shared-vs-last 取新；新增 4 个纯单元测试）；`src/investment_cases.rs`（`instrument_position` 改用同一 shared quote，多组合 slices 统一同一价格）。
  - **otw-core**：`src/market.rs`（新：ensure/gate/TTL/单飞/batch）；`src/main.rs`（`mod market;`）；`src/watchlists/quotes.rs`（刷新成功后 upsert 共享行）；`src/portfolios/prices.rs`（`stock_price_usd`/`china_price_usd` 改 `pub(crate)` 供共享 gate 复用——仍是同一 fetch 实现）；`src/portfolios_api.rs`（list/detail 进门前 ensure）；`src/investment_cases_api.rs`（/live 与 record decision 前 ensure）。
  - **前端**：`frontend/src/lib/modules/portfolios/PortfolioDetail.svelte` —— 资产/操作表所有数字列 header 由 `sortHeader` 统一加 `th.num`（= right），code 列 header/body 一并右对齐（`.code{text-align:right}`，symbol 用 mono），操作列不动；列顺序/数据/排序逻辑不改。
  - 测试：`core/otw-store/tests/market_quotes.rs`（新 6 个 DB 集成测试）。
- **API 变化**：无新 endpoint（只新增行为：读端入口 freshness gate 与共享 cache 优先）。
- **Upstream 潜在冲突点**：低。全部 additive（新表/新模块/新函数）；仅改 `portfolios.rs`/`quotes.rs`/两个 api handler 的读取前步骤。若 upstream 重构 quote 持久化或 Portfolio 计算函数签名需三方合入。
- **如何验证**：`scripts/cargo-check.sh` ✓；`cargo test -p otw-store --lib`（46/46 ✓，含 4 个新 quote-pick）；`cargo test -p otw-store --test market_quotes --test price_observations`（6+7 ✓）；frontend `npm run build` ✓。部署后 API 冒烟（真实会话）：Watchlist 601118=7.87 CNY → GET /api/portfolios 与 GET /api/portfolios/{id}（asset price=7.87、mv/unrealized 按 7.87 重算）→ GET /api/investment-cases/{id}/live（price=7.87，`price_at` 与 portfolio 视图**完全相同**、unrealized 与 portfolio 一致）→ `market_quotes` 出现 `china:601118`/`china:600795`（USD、age≈0）→ 全部 PASS。人工：Watchlist→Portfolio→Research 三处同价同 P&L；Watchlist auto refresh 后切换即用新价；关闭 auto refresh 等待 stale 后直接进 Portfolio/Research 会主动取新价；Decision/Change snapshot 与 Price Observation 快照不受影响（测试 7 回归保护）。
- **targeted tests（覆盖需求 §19）**：fresh shared quote 定价 asset view（#2）；stale/缺失 shared 回落 last price（#5）；Research instrument_position 跨两个组合同一 fresh quote、各 slice 同价（#3/#I 聚合语义）；shared quote 不改 quantity/basis/realized 等交易事实（#1）；identity 级 upsert 单行去重、重复输入容错（#6）；quote 更新不重写已冻结用户快照（#7）；单元：quote 新旧选择 4 例。Research 不依赖 Portfolio refresh side-effect 由 /live 前置 ensure 保证（#4）。
- **如何回滚**：恢复 `otw-core:rollback-pre-marketquote` / `otw-frontend:rollback-pre-marketquote`（= f7144ac673ec / 50fea7a8e548，C-013 部署产物）；DB 侧（如需彻底移除）：`DROP TABLE market_quotes; DELETE FROM _sqlx_migrations WHERE version = 9000000107;`（旧 binary 不内嵌 9000000107）。该表只是可重建缓存，删除后各模块退回旧 last-price 语义，无数据损失。当前 running/latest：core `4a42f653cd97`、frontend `9f226f14fb63`。

---

### C-015 Watchlist 日线序列缓存失效 + A 股会话日边界修正（2026-09-11）

- **修改原因（用户可见）**：601118 在价格观察弹窗「查看每日价格」里看不到昨天（09-10）的收盘价——而 cn-market 实际已返回 09-10。两个根因：
  1. **日线缓存 TTL 永不失效**（`watchlists/quotes.rs`）：`compute_quote` 每次重算都写 `history_at = now`，而 `cached_history` 又用 `history_at` 当“抓取时间”判断 6h TTL，于是序列一旦抓到就永久复用——09-10 的 bar 早已可从 provider 取到，缓存里却停在 09-09（sparkline 与 24h/3d/7d/30d 的参照收盘同样受影响）。
  2. **弹窗按 UTC 日划分会话**（`frontend/.../observations.js`）：A 股日线戳在 15:00 CST（=07:00 UTC），CN 用户 00:00–08:00 属于“次日”，UTC 分组会把刚收盘那一场当成“今日未完成”丢弃，并把 live 点标成次日 → 昨天那一行消失 + 多一行幻影。
- **修改**：
  - 后端：新增 `history_fetched_at`（仅在真正 fetch 时写入），`cached_history` 用它做 TTL；**缺失该字段的旧 quote 视为过期**（部署后一次性自愈，修复被旧时间戳冻死的序列）；`history_at` 保留为“本次计算时间”。`demo_seed` 的示例 quote 同步带该字段（保持“首次真实刷新会重取”的演示行为）。
  - 前端：弹窗序列改为按**市场会话日**分组（china → UTC+8；其他 provider 维持原 UTC 行为），并**当 live 等于最后一根已完成收盘价时不再追加“当前”点**（收盘后避免多出一行“今天=昨天收盘”的幻影行）。
- **修改文件**：`core/otw-core/src/watchlists/quotes.rs`（+2 个回归单测）、`core/otw-core/src/demo_seed.rs`、`frontend/src/lib/modules/watchlists/observations.js`。
- **migration**：无（无 schema 变化）。
- **Upstream 潜在冲突点**：中低。`compute_quote` 签名新增一个参数、`cached_history` 返回类型变化——upstream 若改动 Watchlist 报价缓存需三方合入；弹窗文件为定制新增，冲突面小。
- **如何验证**：`cargo check` ✓；`cargo test -p otw-core --bin otw-core -- watchlists::` 12/12 ✓（含新 `history_cache_tests`：fresh 复用 / 超 TTL 重取 / 旧字段视为过期 / 重算不改抓取戳）；frontend build ✓。部署后 API 触发一次刷新：`history_fetched_at` 出现、`history` 末三根 = 09-10=7.34 / 09-09=8.15 / 09-08=7.87 ✓；用真实 JSON 复现弹窗算法，行序 = 2026-09-11(当前) 6.80、2026-09-10 7.34、09-09 8.15 … ✓。
- **附带发现（未改语义，待用户决定）**：`change_24h` 按既有设计是“滚动 24h”（`ref_at` 用 now−24h 截断），A 股日线戳在 15:00 CST，因此 CN 上午时段它参照的是**上一场之前的收盘**（601118 当时显示 −16.6%，而“对比昨收”应为 −7.4%）。若希望 24h 列改为“相对上一根已完成收盘”，另开一轮小改动（只影响取参照那一行）。
- **如何回滚**：恢复 `otw-core:rollback-pre-quotehistory`（= 4a42f653cd97）/ `otw-frontend:rollback-pre-quotehistory`（= 9eeeda359f83）；无 schema 变更需回退。当前 running/latest：core `1c00fb62ea1d`、frontend `ed51c4ffddfb`。DB 备份：`/opt/apps/backups/otw-backup-pre-quotehistory-20260911-104048.sql`。

### C-016 外部 AI Agent 独立 API Token + `GET /api/agent/ping`（2026-10-29）

- **修改原因**：为外部 AI Agent（例如 ChatGPT 定时任务）提供独立于浏览器登录的受控 API 入口。Phase 1 只验证「外部 AI 能否用独立 Token 访问 OTW API」，不实现查询持仓/下单/决策记录等业务能力，也不修改现有模拟交易逻辑。
- **实现方案（复用现有架构）**：
  - 新增独立表 `agent_api_tokens`：Token 绑定 `users.id`，携带 `agent` 主体标签与 `scopes TEXT[]`，支持 `expires_at` / `revoked_at`；服务端只保存 SHA-256（复用 `otw_store::mcp::hash_token`），明文只在创建命令输出中出现一次。
  - 新增 `GET /api/agent/ping`，挂在 session 中间件之外，只接受 `Authorization: Bearer <API_TOKEN>`；身份只来自 Token 行，不读取任何请求参数或 body。
  - Token 的创建/列表/撤销通过 host CLI（`otw-core` 命令）完成，不新增 HTTP 管理后台。CLI 需要能访问数据库，普通用户仅凭登录会话无法管理或读取 Token。
- **修改文件**：
  - `core/otw-store/migrations/9000000108_agent_api_tokens.sql`
  - `core/otw-store/src/agent_api_tokens.rs`（新模块：哈希、scope、过期/撤销、查找、last_used_at）
  - `core/otw-store/src/lib.rs`（模块注册）
  - `core/otw-core/src/agent_access.rs`（Bearer 解析 + 401/403 + ping handler + 单测）
  - `core/otw-core/src/main.rs`（`mod agent_access` + 公开路由 `/api/agent/ping`）
  - `core/otw-core/src/cli.rs`（`create-agent-token` / `list-agent-tokens` / `revoke-agent-token`）
  - `scripts/agent-api-smoke.sh`（不打印 Token 的 curl 冒烟脚本）
- **数据库 migration**：`9000000108_agent_api_tokens.sql`（只新增表与索引，无 destructive 变更）。
- **API 变化**：
  - `GET /api/agent/ping`（新增，公开路由但必须 Bearer Token）
    - 200：`{"status":"ok","agent":"<token.agent>","serverTime":"<RFC3339>","authenticated":true}`
    - 401：缺失/格式错误/未知/过期/已撤销 Token
    - 403：Token 有效但缺少 `agent:ping` scope
  - 不修改任何现有 API、交易、资金、持仓业务。
- **认证隔离**：
  - session：仅 cookie `otw_session`，表 `sessions`；
  - MCP：`otw_mcp_` 前缀，表 `mcp_tokens`，仅 `/api/mcp`；
  - agent API：`otw_agent_` 前缀，表 `agent_api_tokens`，仅 `/api/agent/*`；
  - 三者表、传输方式、Token 前缀均不同；agent 路由不读 cookie，session 路由不读 `Authorization`。
- **Upstream 潜在冲突点**：低。
  - `main.rs` 公开路由新增一行、`mod` 列表新增一行；`cli.rs` 新增命令分支。
  - 新增 migration 使用私有 `9000000108` 段，不与 upstream 撞号。
  - `otw-store/lib.rs` 新增模块声明；upstream 若新增同名模块需合并。
- **如何验证**（2026-10-29 实际执行）：
  - `scripts/cargo-check.sh` ✓
  - `cargo test -p otw-store --lib agent_api_tokens` 3/3 ✓；`cargo test -p otw-core --bin otw-core agent_access` 3/3 ✓
  - 部署后 `_sqlx_migrations` 出现 `9000000108` ✓；`/api/health` 200 ✓
  - `scripts/agent-api-smoke.sh`：有效 Token 200；无/无效/过期/已撤销 401；无 scope 403；agent Token 访问 session 路由 401；重复请求 200/200 —— 10/10 ✓
  - 临时用户成功登录、cookie 访问 `/api/portfolios` 200、cookie 访问 `/api/agent/ping` 401 ✓
  - ping 前后业务表行数（portfolios/assets/operations）不变 ✓；`agent` 字段随 Token（`otw-verify`）变化 ✓；`serverTime` 为真实 UTC 时间 ✓
  - 测试 Token 已全部删除（`agent_api_tokens` 0 行）。
- **如何回滚**：
  1. `docker compose stop core`；
  2. `DELETE FROM _sqlx_migrations WHERE version = 9000000108;`（若新 binary 已应用）；
  3. `DROP TABLE IF EXISTS agent_api_tokens;`（schema 可保留也无害，但旧 binary 不引用）；
  4. 还原上述文件并重建 Core（旧 binary 编译时不含该 migration，不回退 migration row 会因 `VersionMissing` 拒绝启动）。
  - 注意：本次构建前旧 image 的 layers 已被 Docker 清理，无法用 image tag 直接回滚；需从代码树重建。
- **已知限制 / 安全风险**：
  - Token 管理只有 host CLI，没有 UI；轮换/查看需能执行容器命令。
  - 无失败次数限流（Token 为 256-bit 随机值，在线暴力不可行；格式检查已避免无效请求打到 DB）。
  - `last_used_at` 会在成功 ping 时更新（认证元数据，非业务数据）。
  - `serverTime` 为容器 UTC RFC3339；如需固定 +08:00 展示，应在后续约定时区后再改。
  - MCP Server 能力已存在（`/api/mcp`，generic gateway tools），但尚未把 `ping_otw` 作为独立 MCP tool 暴露；本阶段按要求只完成 REST。

---

### C-017 MCP OAuth 2.1 授权服务器（ChatGPT + Codex，2026-10-30）

- **修改原因**：ChatGPT 云端 MCP connector 要求 OAuth 2.1 + PKCE，不能只靠固定 Bearer；本地 Codex 仍需要兼容原有 `otw_mcp_` 固定 Token。目标是在 OTW 内实现最小完整、可撤销的 OAuth 授权服务器，并让 OAuth Token 只具备**只读** MCP 能力。
- **架构**：
  - OAuth AS 与 MCP Resource Server 均在 `otw-core` 内，无需独立部署组件。
  - Authorization Server 使用不透明 Token（不是 JWT）：access / refresh / authorization code / CSRF 均为 256-bit OS 随机值，数据库只存 SHA-256；无新增密码学依赖。
  - 客户端注册：实现 **DCR（RFC 7591）** 与 **host CLI 预注册**；未实现 CIMD。当前 MCP spec（2026-07-28）推荐 CIMD、把 DCR 标记为 deprecated 但保留兼容；预注册是三种机制之一，ChatGPT connector 的手工 Client ID 流程可直接使用预注册。DCR 主要服务 Codex / 自动发现客户端。若未来 ChatGPT 强制要求 CIMD，需要追加 CIMD（含 HTTPS 元数据抓取、SSRF 防护与缓存）。
  - 发现：`/.well-known/oauth-protected-resource`（含 `/api/mcp` 路径变体）与 `/.well-known/oauth-authorization-server`（含路径变体）由 Core 提供；`/.well-known/openid-configuration` 返回 JSON 404（不伪装成 OIDC）。
  - 外部地址只来自 `OTW_PUBLIC_BASE_URL`；不使用 Host / X-Forwarded-* 生成安全 URL。仅接受 HTTPS，loopback HTTP 仅供本地测试。
- **授权与安全**：
  - Authorization Code + **强制 PKCE S256**；`plain`/缺失 challenge 拒绝。
  - `client_id`、`redirect_uri`、`resource`、scope 在授权请求、consent、code 存储、token exchange 全链路绑定；code 只能使用一次。
  - consent 页面由服务端渲染，HTML 转义；pending request 一次性消费，CSRF Token 只存哈希；没有开放重定向（redirect_uri 必须精确匹配注册值）。
  - access token 15 分钟；refresh token 30 天、每次使用轮换；同一 `family_id` 的 refresh token 被重复使用时，整族 access/refresh 立即撤销（replay detection）。
  - 支持 RFC 7009 revocation；用户可通过 `DELETE /api/oauth/grants/{client_id}`（session 保护）或 host CLI `revoke-oauth-grants` 撤销全部授权；密码修改/重置也会撤销该用户的全部 OAuth Token。
  - Token 校验：过期、撤销、客户端禁用、`resource` 不匹配、缺少 `mcp:read`、非 admin 主体均拒绝；401/403 返回正确的 `WWW-Authenticate`（含 `resource_metadata`、`scope`、`error`）。
- **权限边界（重要）**：
  - OTW 当前是**单用户数据模型**：`portfolios` / `watchlists` 等业务表没有 `user_id`，API 也不按用户过滤。因此本阶段**不宣称**实现了用户/投资组合隔离。
  - OAuth 授权只允许 **admin（当前唯一 owner）**；非 admin 在 authorize 页面直接 403。OAuth Token 绑定真实 `users.id`，MCP 内部调用也用该用户身份注入（`internal_call::call_as`），而不是默认 admin。
  - scope 映射：`mcp:read` → 所有 catalog 模块 `"r"`；`offline_access` 只影响 refresh token，不授予资源访问。未知 scope 一律拒绝。OAuth 的 `tools/list` 只暴露 `otw_catalog` / `otw_read`，`otw_write` / `otw_compute` 既不在列表中，手工调用也会被拒绝。
  - 固定 `otw_mcp_` Token 完全保留原有行为（模块权限 + admin 内部身份），用于本地 Codex 兼容。
- **修改文件**：
  - `core/otw-store/migrations/9000000109_mcp_oauth.sql`
  - `core/otw-store/src/oauth.rs`（新）
  - `core/otw-store/src/lib.rs`（模块注册；密码变更/重置时撤销 OAuth）
  - `core/otw-core/src/oauth.rs`（新：发现、authorize/consent、token、DCR、revoke、grants、PKCE/URI/scope 校验）
  - `core/otw-core/src/mcp/mod.rs`（固定 Token + OAuth 双认证、401 challenge、只读 principal、OAuth 工具限制）
  - `core/otw-core/src/internal_call.rs`（`call_as` 注入真实 OAuth 用户）
  - `core/otw-core/src/main.rs`（模块/路由/`OTW_PUBLIC_BASE_URL`/OAuth 清理任务）
  - `core/otw-core/src/cli.rs`（OAuth client 预注册/列表/删除、OAuth grants 撤销）
  - `deploy/Caddyfile`（`/.well-known/*` → core，避免 SPA fallback 返回 HTML）
  - `deploy/docker-compose.yml` / `deploy/.env.example`（`OTW_PUBLIC_BASE_URL`）
  - `scripts/oauth-smoke.py`（不打印 Token 的端到端 OAuth/MCP 冒烟测试）
  - `core/otw-store/tests/oauth.rs`（code 绑定/单次使用/过期、refresh 轮换/重放、授权撤销）
- **数据库 migration**：`9000000109_mcp_oauth.sql`，仅新增 5 张表（`oauth_clients`、`oauth_pending_requests`、`oauth_authorization_codes`、`oauth_access_tokens`、`oauth_refresh_tokens`），无 destructive 变更。
- **API 变化**：
  - 新增 discovery：`/.well-known/oauth-protected-resource`、`/.well-known/oauth-authorization-server`（及路径变体）。
  - 新增 OAuth：`/api/oauth/authorize`、`/api/oauth/authorize/decision`、`/api/oauth/token`、`/api/oauth/register`、`/api/oauth/revoke`。
  - 新增 session 保护：`GET /api/oauth/grants`、`DELETE /api/oauth/grants/{client_id}`。
  - `/api/mcp` 鉴权扩展为固定 Token 或 OAuth access token；现有固定 Token 行为不变。
- **部署（已执行）**：
  1. `deploy/.env` 增加 `OTW_PUBLIC_BASE_URL=https://nova.lumora-lab.top`（gitignored，不进仓库）。
  2. `docker compose ... build core`，`up -d --no-deps core`；新 Core 自动应用 `9000000109`。
  3. 重建 Caddy 容器以加载 `/.well-known/*` 规则（编辑后 inode 变化，reload 不够；`--force-recreate` 生效）。
  4. 公网验证：PRM/ASM 返回 `application/json` 200；OIDC probe 404 JSON；`POST /api/mcp` 无 Token 返回 401 + `WWW-Authenticate`。
- **测试（实际执行，2026-10-30）**：
  - `scripts/cargo-check.sh` ✓。
  - 单元测试：`oauth` 5 个（PKCE 已知向量、redirect URI、public base、scope、HTML escape）✓；MCP 31 个 ✓。
  - 隔离测试库 `otw_oauth_test` 上 `cargo test -p otw-store --test oauth` 5/5 ✓（code 绑定/单次使用/过期、refresh 轮换/重放撤销、用户级撤销）；测试后已 DROP DATABASE。
  - 公网端到端 `scripts/oauth-smoke.py`：**40 passed, 0 failed**，覆盖 DCR、consent、PKCE、code 单次使用、wrong PKCE、redirect/resource/client 不匹配、OAuth initialize/tools/list/只读调用、OAuth 写入被拒、refresh 轮换与重放、固定 Token 兼容、revocation。
  - 额外定向测试：过期/撤销/scope 不足 Token 分别 401/401/403 且 `WWW-Authenticate` 正确；OAuth Bearer 访问 session 路由 401；session Cookie 访问 `/api/mcp` 401；`DELETE /api/oauth/grants/{client_id}` 后 Token 立即 401；非 admin authorize 403。
  - 测试账号、DCR clients、测试 Token 已全部清理；`oauth_*` 表归零。
  - **未验证**：ChatGPT 实际 connector 连接与定时任务（需要 ChatGPT 侧操作，且定时任务需单独验证）。ChatGPT 文档站对本服务器 IP 返回 403，本次以 MCP 2026-07-28 spec + 已知 connector 行为实现；未实现 CIMD。
- **如何回滚**：
  1. `docker compose stop core`；
  2. `DELETE FROM _sqlx_migrations WHERE version = 9000000109;`
  3. `DROP TABLE IF EXISTS oauth_access_tokens, oauth_refresh_tokens, oauth_authorization_codes, oauth_pending_requests, oauth_clients;`
  4. 还原上述代码并重建 Core。旧 binary 若不回退 migration row 会因 `VersionMissing` 拒绝启动；旧 image 可能已被 Docker 清理，需从代码树重建。
- **Upstream 潜在冲突点**：中低。`main.rs` 新增模块/路由/AppState 字段；`mcp/mod.rs` 的 `tools_list`/`tools_call`/`run_endpoint` 签名与认证路径被重构（upstream 若大改 MCP 需三方处理）；`internal_call.rs` 新增 `call_as`；`cli.rs` 新增命令；Caddyfile 新增 well-known 路由。migration 使用私有 `9000000109` 段。
- **已知限制 / 安全风险**：
  - 没有用户/投资组合隔离（schema 不具备）；只允许 admin，且 admin 可读全部数据。若未来需要多用户/限定组合，必须给业务表加 ownership 并在所有列表/详情/聚合/子资源处强制过滤，不能只过滤顶层列表。
  - 无 CIMD；DCR 为兼容保留。若 ChatGPT 后续强制 CIMD，需要追加实现。
  - 没有 OAuth 授权的 Settings UI；用户撤销通过 API 或 host CLI。
  - 未做 per-IP 的令牌端点限流（DCR 有全局限流与 client 上限）；Token 为 256-bit 随机值，在线暴力不可行。
  - 固定 MCP Token 在本次任务前已被用户在 UI 中撤销（app_logs 2026-10-29 15:16:50 UTC），本地 Codex 需要重新创建一个 `otw_mcp_` Token，或改用 OAuth。
  - OAuth 端点只支持 public client（`token_endpoint_auth_method=none`）；不支持 confidential client secret。

---

### C-018 AI Paper Trading Agent（2026-11-01，开发中）

- **目标**：让 ChatGPT / 外部 AI 在 OTW 内运行独立模拟交易策略，记录每轮事实、预期、推断和风险判断，并在明确授权后执行模拟买卖；不连接真实券商。
- **架构边界**：
  - 一个 Paper Trading strategy 对应一个独立 Portfolio Tracker portfolio。
  - strategy 保存初始资金、策略/模型元数据、费用/风险限制，并支持显式 `start_date`（可回填实验真实起始日）。
  - Portfolio Tracker 的 `portfolio_operations` 仍是唯一成交事实账本；Paper Trading 不维护第二份持仓。
  - 可用现金 = 初始资金 + Portfolio operations 的现金流，按操作日期的 Journal FX 转换到策略币种；不维护第二个可变现金余额。
  - `paper_trading_runs` 保存 AI 分析与服务端冻结的账户快照；`paper_trading_orders` 只保存委托意图、报价来源和 Portfolio operation 关联。
  - Research 的跨 Portfolio 持仓聚合排除 paper portfolios；普通 Portfolio API / 导入路径拒绝修改 paper portfolio 的成交账本。
- **安全执行**：
  - 下单 body 不接受成交价或手续费；服务端读取 Shared Market Quote，校验报价新鲜度，应用策略滑点和费用配置。
  - 服务端按策略行锁串行化下单；校验 run 与 strategy 归属、决策方向与订单方向、幂等键、可用现金、long-only 可卖数量、单笔金额和单标的仓位上限；China A-share stocks 额外执行 T+1 可卖数量约束。
  - 订单与 Portfolio operation 在同一数据库事务提交；重复请求返回原订单，复用同一幂等键但提交不同订单内容则拒绝。
  - 默认费用是假设值：佣金 3 bps、最低佣金 5（资产币种）、卖出额外 levy 5 bps、滑点 5 bps；均可在创建策略时调整，不宣称覆盖所有市场真实费率。
- **OAuth / MCP**：
  - 新增可选 scope `mcp:paper-trade`，必须在 OAuth consent 中明确授权。
  - `mcp:read` 仍是默认只读；`mcp:paper-trade` 只将 MCP Catalog 的 `paper-trading` 模块提升为 `rw`，普通 `portfolios` 及其他模块仍为 `r`。
  - OAuth 仍不可删除、不可调用 `otw_compute`；策略创建是 session-only，不加入 MCP Catalog。
- **主要文件**：
  - `core/otw-store/migrations/9000000110_paper_trading.sql`
  - `core/otw-store/src/paper_trading.rs`、`core/otw-store/src/lib.rs`
  - `core/otw-core/src/paper_trading_api.rs`、`main.rs`
  - `core/otw-core/src/mcp/catalog.rs`、`mcp/mod.rs`
  - `core/otw-store/src/oauth.rs`、`core/otw-core/src/oauth.rs`
  - `core/otw-store/src/investment_cases.rs`、`investment_cases_api.rs`
  - `core/otw-core/src/portfolios_api.rs`、`portfolios_import_api.rs`
  - `CUSTOM_ARCHITECTURE.md`
- **Migration**：新增 `9000000110_paper_trading.sql`，创建 strategy / run / order 三张表；不改写既有 Portfolio ledger。
- **API**：
  - Session-only：`POST /api/paper-trading/strategies` 创建策略和对应 Portfolio（可指定历史 `start_date`）；`PATCH /api/paper-trading/strategies/{id}` 暂停/恢复/结束策略（ended 不可恢复）。
  - MCP 可读：策略列表/详情/context、标的搜索、runs、orders。
  - MCP paper-trade scope 可写：添加资产、记录 AI run、提交模拟订单。
- **验证状态**：当前只在 `feature/paper-trading-agent` 分支提交代码和单元测试；**尚未运行 Rust 编译/测试、尚未应用数据库 migration、尚未部署**。需要通过项目规定的 `scripts/cargo-check.sh` 与目标测试后才可部署。
- **回滚**：部署前可直接丢弃分支。若未来已应用 migration，先停止 Core，在事务中删除 `_sqlx_migrations` 的 version `9000000110`，再删除 `paper_trading_orders`、`paper_trading_runs`、`paper_trading_strategies` 三张表，恢复旧代码并重建 Core。
- **Upstream 潜在冲突**：中等。涉及 `main.rs` 路由、MCP Catalog / OAuth scope 与权限映射、Portfolio API 防护、Investment Case 聚合查询；后续 upstream 若改动这些模块需三方合并。

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
| 9000000102 | `9000000102_investment_cases.sql` | Investment Case MVP：`investment_cases` + `decision_records`（见 C-007） | ✅ 已应用 |
| 9000000103 | `9000000103_investment_case_interaction.sql` | Investment Case 三层模型：`investment_theses` / `thesis_assumptions` + 决策记录扩展（见 Phase 1.5 提交） | ✅ 已应用 |
| 9000000104 | `9000000104_investment_case_document_link.sql` | 研究文章与案例解绑（`document_id` 可空 + FK SET NULL）+ 存量文章移入「股票研究」目录（见 C-008） | ✅ 已应用 |
| 9000000105 | `9000000105_investment_case_round_state.sql` | 每轮判断携带估值/最大问题（`decision_records.valuation_view`/`biggest_question`）+ case 级字段降级为最新记录投影（见 C-011） | ✅ 已应用 |
| 9000000106 | `9000000106_watchlist_price_observations.sql` | Watchlist 价格观察：`watchlist_price_observations`（用户快照；item FK ON DELETE CASCADE，见 C-013） | ✅ 已应用 |
| 9000000107 | `9000000107_market_quotes.sql` | 共享 Market Quote cache（`(provider, provider_id)` 一行 USD 当前价；Watchlist/Portfolio/Research 共享，见 C-014） | ✅ 已应用 |
| 9000000108 | `9000000108_agent_api_tokens.sql` | 外部 AI Agent API Token（绑定用户 + scopes + 过期/撤销；见 C-016） | ✅ 已应用 |
| 9000000109 | `9000000109_mcp_oauth.sql` | MCP OAuth 2.1：clients / pending / codes / access / refresh tokens（见 C-017） | ✅ 已应用 |
| 9000000110 | `9000000110_paper_trading.sql` | AI Paper Trading：strategy / run / order audit tables（见 C-018） | ⏳ 分支开发中，未应用 |

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
- **merge commit**：`6b7b955b2ebb2c2639a18445a250475277635493`（custom/main merge vendor/upstream；冲突 2 个 i18n 文件已按 §8.3 解决）。
- **数据库**：新 Core 启动时自动应用 `0092–0097`（installed_on `2026-09-01 14:12:46 UTC`，success=t）；`9000000100/101` checksum/installed_on 原值未变。
- **镜像**：新 core `a5df3007…`（v0.0.9）；新 frontend `23fcd0d6…`；rollback tags `otw-core:rollback-pre-v0.0.9` / `otw-frontend:rollback-pre-v0.0.9`。
- **备份**：`/opt/apps/backups/otw-db-pre-v0.0.9-20260901-220807.sql`（136M）。
- **验证**：cargo check ✓、otw-store 34 tests ✓、otw-core 444 passed ✓、frontend build ✓、冒烟全链路 200（login/watchlist 601118+refresh/portfolio 601118 avg_cost 6.49 unrealized +60.0/editor create-patch-read-delete/automator workflows+schedules+catalog）。
