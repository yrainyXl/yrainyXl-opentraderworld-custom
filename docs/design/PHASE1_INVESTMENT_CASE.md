# Phase 1 — Investment Case MVP 设计 v2（2026-09-01，强制修订版）

> 状态：**设计稿 v2（待批准）**。未创建 migration、未改数据库、未构建/部署。
> 关联文档：`CUSTOM_ARCHITECTURE.md` §5.4/§5.5、§11 Phase 1；`CUSTOM_CHANGES.md` §3（migration 9000000102+）。
> v2 修订：User ownership、MVP 创建 API 收缩、Editor ownership、live 聚合/FX 语义、live price 选择策略、snapshot 字段、schema 简化、API 最小化、路由命名、测试补充。

## 核心业务流

```
Portfolio asset row（如 china/601118 海南橡胶）
  → 点击「研究」
  → POST /api/investment-cases { portfolio_asset_id }
       ├─ 已存在 → 返回 existing case（幂等，无新 document）
       └─ 不存在 → 事务内创建 document + case（并发安全，无 orphan）
  → /investment-cases/{id} 页面
```

## 0. Ownership 模型审查结论（先查证，不猜测）

- `users(id uuid, username, password_hash, is_admin, ...)`；`sessions.user_id → users(id) ON DELETE CASCADE` —— **这是 OTW 唯一的 user FK 约定**。
- `portfolios` / `documents` / `watchlist_items` **均无 user 列**：OTW 当前是**单用户应用**（`documents.rs` 注释 "Single-user: no owner scoping"；require_auth 经 session → 注入 `User` 到 request extensions，无 handler 做过 ownership 校验）。
- **结论**：Investment Case 引入 `user_id`，FK 约定完全沿用 `sessions` 模式（`uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE`）；唯一性 = `(user_id, provider, provider_id)`。
- **portfolio asset「属于当前用户」语义**：单用户模型下全部 portfolios 即当前用户资产；ensure 校验 = asset 存在性 + 通过 `User` extension 取当前用户。多用户扩展路径（后续 migration 给 `portfolios` 加 `user_id`）在设计中预留，Phase 1 不做。
- **所有 Case/Decision handler 必须**从 request extensions 取 `otw_store::User` 做 ownership 校验（不存在/不匹配 → `ApiError::forbidden`），**不能只依赖 require_auth**（require_auth 只保证"已登录"）。

## A. 数据模型

| 实体 | 职责 | 权威来源 |
|---|---|---|
| `investment_cases` | 用户 × instrument 一个 case；关联 Editor 文档；身份元数据（创建时从 asset 行解析，非前端提交） | 本模块 |
| `decision_records` | 决策时间线；创建时冻结 snapshot | 本模块（snapshot 现算，创建后不可变） |
| 持仓/盈亏/operations | **不落库**，实时读取 Portfolio 计算 | Portfolio Tracker（唯一 SSOT） |
| 研究正文 | 复用现有 documents/Editor | Editor（唯一 SSOT） |

## B. 表结构（migration `9000000102_investment_cases.sql`）

```sql
CREATE TABLE investment_cases (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,  -- 沿用 sessions FK 约定
    provider     text NOT NULL,              -- 'china' | 'coingecko' | 'yahoo' ...
    provider_id  text NOT NULL,              -- '601118'
    symbol       text NOT NULL DEFAULT '',
    name         text NOT NULL DEFAULT '',
    exchange     text NOT NULL DEFAULT '',
    currency     text NOT NULL DEFAULT 'USD',
    asset_class  text NOT NULL DEFAULT 'equity',
    document_id  uuid NOT NULL REFERENCES documents(id) ON DELETE RESTRICT,
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, provider, provider_id)
);
-- 注：MVP 无 close/archive 语义 → 不设 status 列；Phase 2 需要时以新 migration 添加。
-- 注：无 instrument_id 列（Phase 1 无统一 Instrument Service）；迁移路径见 §H。

CREATE TABLE decision_records (
    id                      uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    investment_case_id      uuid NOT NULL REFERENCES investment_cases(id) ON DELETE CASCADE,
    decision_type           text NOT NULL,
    confidence              smallint NOT NULL DEFAULT 3,
    reason                  text NOT NULL DEFAULT '',
    evidence                text NOT NULL DEFAULT '',
    counter_evidence        text NOT NULL DEFAULT '',
    invalidation            text NOT NULL DEFAULT '',
    watch_items             text NOT NULL DEFAULT '',
    -- ── 冻结快照（创建后永不更新；无 UPDATE 路径）──
    recorded_at             timestamptz NOT NULL DEFAULT now(),
    snapshot_provider       text NOT NULL,
    snapshot_provider_id    text NOT NULL,
    snapshot_symbol         text NOT NULL,
    snapshot_name           text NOT NULL,
    snapshot_exchange       text NOT NULL DEFAULT '',
    snapshot_currency       text NOT NULL,
    snapshot_price          double precision,
    snapshot_price_at       timestamptz,          -- 报价时间点（live price 选择策略的 last_price_at）
    snapshot_quantity       double precision NOT NULL DEFAULT 0,   -- 原 snapshot_position
    snapshot_avg_cost       double precision,
    snapshot_cost_basis     double precision,
    snapshot_market_value   double precision,
    snapshot_unrealized_pnl double precision,
    snapshot_unrealized_pct double precision,
    CHECK (decision_type IN ('observe','hold','buy','add','reduce','exit')),
    CHECK (confidence BETWEEN 1 AND 5)
);
CREATE INDEX idx_decision_records_case ON decision_records(investment_case_id, recorded_at);
```

v2 变更：`user_id` + 唯一键改 `(user_id, provider, provider_id)`；删 `status`、删 `linked_operation_id`（不为 Phase 2 预留无 FK 字段）；`confidence` 改 `smallint`；`snapshot_position → snapshot_quantity`；新增 `snapshot_price_at`、`snapshot_cost_basis`。

## C. API（MVP 最小集，全部经 require_auth + handler 内 ownership 校验）

| 方法/路径 | 请求 | 说明 |
|---|---|---|
| `POST /api/investment-cases` | `{portfolio_asset_id}` | **ensure**：校验 asset 存在（单用户语义即属当前用户）→ 后端从 asset 行解析 provider/provider_id/symbol/name/exchange/currency/asset_class（**不信任前端事实字段**）→ 已存在返回 existing case；否则事务创建 document + case → `{case}` |
| `GET /api/investment-cases/{id}` | — | ownership 校验 → `{case}`（含 `document_id`；**不代理 document content**，正文走现有 documents API） |
| `GET /api/investment-cases/{id}/live` | — | ownership 校验 → 实时聚合（§F/§G）→ `{instrument, price, price_at, currency, position{quantity, avg_cost, cost_basis, market_value, unrealized_pnl, unrealized_pct}, portfolios:[{id,name,quantity,avg_cost,market_value,unrealized_pnl}], operations:[{asset_id, portfolio_id, portfolio_name, side, quantity, price, fee, currency, op_date}]}` |
| `GET /api/investment-cases/{id}/decisions` | — | ownership 校验 → 时间线（含全部 snapshot 列） |
| `POST /api/investment-cases/{id}/decisions` | `{decision_type, confidence, reason, evidence, counter_evidence, invalidation, watch_items}` | ownership 校验 → 后端用与 /live 相同函数现算 snapshot 一次写入 → `{decision}`；**不接受前端 snapshot 数字；无 UPDATE API** |

不提供：provider/provider_id query API（入口 = POST ensure）；case 删除/关闭/重命名/列表。

## D. 前端页面/组件结构

```
frontend/src/routes/investment-cases/[id]/+page.svelte   （新；路由理由见 §R）
├─ Header：symbol / name / exchange / currency（静态，来自 case 行）
├─ 实时块（GET /live）：price(+price_at)、quantity、avg cost、market value、unrealized
│    涨红跌绿沿用 isChina/isChinaItem 风格 + fmtMoney/fmtSignedMoney（现有 helper）
├─ Operations 表：side / qty / price / op_date（+portfolio 名）
├─ [记录决策] → DecisionModal.svelte（新）：6 类型 + confidence 1–5 + 5 文本域
├─ Decision 时间线：recorded_at + type + confidence + 冻结 snapshot（标注"当时"）
└─ 研究正文：内嵌现有 Editor.svelte（bind:this + setContent/getJSON
      + autosave → PATCH /api/documents/{id}，复用 routes/editor 模式）

frontend/src/lib/modules/investment-cases/api.js        （新：ensure/getLive/addDecision/listDecisions）
frontend/src/lib/modules/investment-cases/DecisionModal.svelte （新）
frontend/src/lib/modules/portfolios/PortfolioDetail.svelte   （asset 行 ops 单元格加「研究」→ ensure → goto）
frontend/src/lib/modules/registry.js                    （可选：模块项 base /investment-cases）
frontend/src/lib/i18n/en.json / zh.json                 （新 keys，沿用 custom 模式）
```

## E. 与 Editor 的关联方式

- Case 创建事务内创建 `documents` 行（kind=`page`，parent_id=NULL，title=`"<symbol> <name> 研究"`）。
- **复用现有 document creation domain logic**：把 `otw_store::documents::create(pool, ...)` 重构为 tx-compatible 核心 `create_with<E: Executor<'_, Database=Postgres>>(executor, parent_id, kind, title)`（MAX(position) + INSERT 均走 executor），现有 `create(pool, ...)` 变薄包装；case 事务用 `create_with(&mut *tx, ...)`。**不复制 documents 初始化逻辑**。
- `document_id` FK 保持 `ON DELETE RESTRICT`；Editor 删除被引用文档时：`DELETE /api/documents/{id}` handler 先查 `investment_cases WHERE document_id=$1`（store 新 fn），命中 → **`ApiError::conflict`（HTTP 409）**，消息说明先删除/解除 case，绝不暴露 DB FK 500。
- GET case 只返回 `document_id`；正文内容/保存继续走 `GET/PATCH /api/documents/{id}`（Editor = document content SSOT）。
- 动态数据（价格/持仓/盈亏）不写入 content JSON（架构 §5.4 红线）。

## F. Live 聚合与财务口径（含 FX 语义，重点）

`investment_cases::instrument_position(pool, user_id, provider, provider_id)`：

1. 查当前用户全部 `portfolio_assets WHERE provider=$1 AND provider_id=$2`（跨 portfolio；每 portfolio 该 instrument 至多一行，有 UNIQUE 约束背书）。
2. 每个 asset 复用 `portfolios::position_usd`（USD ledger：每笔 op 按 **op_date** FX 折算 USD；买费入 basis、卖费减 proceeds）+ `journal_fx::convert`（USD→instrument 币种，**今日** FX）—— 与现有 `asset_view` **完全相同**的财务口径，只是目标币种 = instrument 币种而非 portfolio 币种。
3. 聚合（instrument 币种）：`quantity=Σ`、`cost_basis=Σ`、`avg_cost=Σbasis/Σqty`、`market_value=price×Σqty`、`unrealized_pnl=MV−Σbasis`、`unrealized_pct=unrealized/Σbasis`；同时返回 per-portfolio 明细。
4. operations 用 `portfolios::list_operations` 合并，**必须带 portfolio_id + portfolio_name**。

**FX 语义（接受用例 C 的关键）**：OTW 既有口径 = **USD ledger**（逐笔按成交日 FX 进 USD，再按今日 FX 折回显示币种）。Portfolio 详情页显示的 avg cost 就是这个口径（`asset_view`），它会随今日 FX 微动，**不是** "历史 native avg cost"。Investment Case **必须与 Portfolio 完全一致**（接受用例 A 要求数字相等），因此：
- 采用同一 USD ledger 口径，不引入第二套计算标准；
- 设计文档明确标注：该 avg cost 是 "USD-basis ÷ 今日 FX" 的显示值，**不宣称**为历史 native 成本；
- USD 资产（FX=1）两种口径恒等；CNY 资产的 native 与 ledger 差异是既有产品语义，不在 Phase 1 改变。

**接受用例**：
- A：单 portfolio 601118 → live 数字与 Portfolio detail 完全一致（同一函数族）。
- B：两 portfolio 同持 → quantity/basis/P&L 正确聚合；operations 合并且带 portfolio 标识。
- C：买入后 FX 变化 → 聚合值跟随 Portfolio 同口径变化（两处一致），语义明确为 ledger 显示值，不伪装 native。

**代码改动**：`portfolios.rs` 的 `position_usd`/`asset_view` 由私有改 `pub(crate)`（仅可见性，零逻辑变化）。

## G. Live price 选择策略

- 多个 portfolio_assets 都有 `last_price` 时：**取 `last_price_at` 最新的一条**（同一价格源的统一口径；若未来有统一 market quote abstraction 再切换，Phase 1 不引入）。
- `live` 返回 `price` + `price_at`（被选 asset 的 last_price_at）+ `currency`（instrument 币种）。
- Phase 1 **不新增主动 quote refresh trigger**（沿用 Portfolio 既有 refresh/auto-refresh；case 页展示的是最后报价时刻）。
- 无任何报价 → price/price_at 为 null，前端显示 —。

## H. Decision snapshot 语义

- 创建时后端调用与 `/live` **完全相同**的 `instrument_position`（同一函数、一次计算），将结果冻结写入 `decision_records`：
  `recorded_at`、`snapshot_provider/provider_id/symbol/name/exchange/currency`、`snapshot_price`、`snapshot_price_at`、`snapshot_quantity`、`snapshot_avg_cost`、`snapshot_cost_basis`、`snapshot_market_value`、`snapshot_unrealized_pnl`、`snapshot_unrealized_pct`。
- **不接受 frontend 提交任何 snapshot 数字**。
- 代码层面**不存在**更新 snapshot 的路径（无 UPDATE 函数/API）→ 之后 Portfolio/行情变化永不改快照（架构 §4.3）。
- UI 严格区分：实时块每次加载重拉 `/live`；时间线数字来自 snapshot 列，标注"当时"。
- 空持仓：`snapshot_quantity=0`，avg_cost/cost_basis/market_value/unrealized/pct/price/price_at 为 NULL。

## I. Instrument identity 方案

- Phase 1 主键：`(user_id, provider, provider_id)`（provider+provider_id 与 watchlist_items / portfolio_assets 同构）。
- 身份冗余列在 ensure 时**从 portfolio_asset 行 server-side 解析**（asset.symbol/name/exchange/currency/asset_class），**不信任前端**。
- 未来：统一 Instrument Service 建立 `instrument_id` 后，加可空列 → 回填 → 收紧唯一键（表结构已预留此路径）。

## J. migration 9000000102 设计

即 §B DDL。风格检查：新增两表 + CHECK/UNIQUE/索引；不触碰既有对象；编号 9000000102（custom 段，与 0097/9000000100/101 无交集）。**不 ALTER 既有表**（documents/portfolios 零改动；documents 删除 409 是 API 层行为，不是 schema 变更）。

## K. 预计修改文件

| 区域 | 文件 | 动作 |
|---|---|---|
| 后端 | `core/otw-store/migrations/9000000102_investment_cases.sql` | 新增 |
| 后端 | `core/otw-store/src/investment_cases.rs` | 新增（case/decision CRUD + instrument_position + find_by_document_id） |
| 后端 | `core/otw-store/src/portfolios.rs` | `position_usd`/`asset_view` → `pub(crate)`（仅可见性） |
| 后端 | `core/otw-store/src/documents.rs` | 重构出 `create_with<E: Executor>`（tx-compatible）；`create` 变薄包装 |
| 后端 | `core/otw-store/src/lib.rs` | `pub mod investment_cases` |
| 后端 | `core/otw-core/src/investment_cases_api.rs` | 新增（5 端点 + ownership 校验） |
| 后端 | `core/otw-core/src/documents.rs` | delete handler：被 case 引用 → 409 |
| 后端 | `core/otw-core/src/main.rs` | merge 路由 |
| 前端 | `frontend/src/routes/investment-cases/[id]/+page.svelte` | 新增 |
| 前端 | `frontend/src/lib/modules/investment-cases/{api.js,DecisionModal.svelte}` | 新增 |
| 前端 | `frontend/src/lib/modules/portfolios/PortfolioDetail.svelte` | 加「研究」入口 |
| 前端 | `frontend/src/lib/modules/registry.js` | 可选模块项 |
| 前端 | `frontend/src/lib/i18n/{en,zh}.json` | 新 keys |
| 文档 | `CUSTOM_CHANGES.md`（C-007）、`CHANGELOG.md` [Unreleased] | 更新 |

## L. MVP 范围

1. Portfolio asset 行 →「研究」→ ensure（幂等、并发安全）→ `/investment-cases/{id}`
2. Case 页：instrument 信息 + 实时块（live）+ operations（带 portfolio 标识）+ 决策时间线 + 记录决策 + 内嵌 Editor 研究正文
3. 决策 6 类型 + confidence(smallint 1–5) + 5 文本字段；创建即冻结 snapshot（含 price_at/cost_basis）
4. Ownership：user_id 落库 + 所有 handler 校验；document 被引用删除返回 409

## M. 明确不做（Phase 1）

- AI 自动分析 / Agent / MCP / Automator / News / Journal 自动同步 / Backtest / 自动交易
- case 列表页、删除/关闭/重命名、status、provider/provider_id query API
- Watchlist「开始研究」入口（非必需）
- 价格刷新触发器（沿用 Portfolio 既有 refresh）
- Research 文件夹归类；多 portfolio 明细 UI 折叠（后端已聚合）
- 多用户 portfolio 隔离（portfolios.user_id 属后续 migration）

## N. 测试计划

**store 单测（otw-store）**
1. `instrument_position`：跨 portfolio 聚合；buy/sell 混合 + fee；CNY→USD 逐笔 FX；空持仓/全卖出
2. 决策创建：snapshot = 当时聚合值；随后 add operation / 改 last_price → snapshot **完全不变**（含 snapshot_price_at）
3. 幂等 + 并发：同 (user, provider, provider_id) 并发 ensure → 恰好 1 case + 1 research document（无 orphan）
4. `create_with` tx 回滚：case 插入失败 → document 不落库

**API 层（otw-core 模块内测试）**
5. 未登录（无 session）→ 401（require_auth 兜底）
6. **user A 无法读取 user B 的 case**（伪造第二个 user 的 case → A 的 GET /{id} → 403/404）
7. **user A 无法 ensure user B 的 portfolio asset**（隔离语义下 = asset 不存在 → 404）
8. decision_type / confidence 越界 → 400
9. **删除被 case 引用的 document → 409**（明确消息，非 500）
10. snapshot 无 UPDATE 路径（编译期无更新函数）

**手工冒烟（部署后）**
11. 601118 入口 → case+document 创建；实时块与 Portfolio 页**完全一致**（单 portfolio，接受用例 A）
12. 第二 portfolio 加同 instrument → 聚合正确（接受用例 B）；operations 带 portfolio 名
13. 记录决策 → 刷新价格/FX 后 snapshot 不变（接受用例 C + 冻结语义）
14. Editor 保存 → 重开内容在；删除被引用文档 → 409 提示

**流程**：`scripts/cargo-check.sh` → targeted tests → 通过后一次性 build/deploy（AGENTS.md 纪律）。

## O. Phase 2 扩展点

- `investment_cases.instrument_id`：统一 Instrument Service 迁移（§I）
- `portfolios.user_id`（多用户隔离）或沿用单用户模型的共享语义
- MCP catalog 增补 case/decision 读取（Read），AI 总结/挑战（Phase 3）
- Journal 从决策派生"当时判断"（Portfolio=交易事实、Journal=统计分析，架构 §5.6）
- case 状态机（open/closed）、case 列表页、Watchlist「开始研究」
- News/宏观证据层挂 case
- 统一 market quote abstraction（替换 §G 的 last_price_at 选择策略）
- 若 Phase 2 需要决策↔操作互链：以新 custom migration 添加 `linked_operation_id`（带 FK）

## R. 路由命名理由

采用 `/investment-cases/[id]` 而非 `/cases/[id]`：
- 模块命名与 registry 条目（`investment-cases`）、API 路径（`/api/investment-cases`）三者一致；
- 未来列表页/其他入口直接挂 `/investment-cases` 下，无重命名成本；
- `/cases` 过于泛化（与 community-docs、backtest 等 "case" 概念混淆），且当前 `routes/` 下已有 `/community-docs` 等全名路由惯例。
