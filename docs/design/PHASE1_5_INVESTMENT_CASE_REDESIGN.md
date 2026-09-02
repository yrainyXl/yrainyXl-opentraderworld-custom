# Phase 1.5 — Investment Case 三层模型重设计（2026-09-02，设计稿）

> 状态：**设计稿，未批准前不编码、不动数据库**。
> 新核心模型：**Investment Thesis（低频）→ Current State（当前状态）→ Change Log（高频）**。
> Research Editor 降级为「补充研究笔记」，不承担主流程。

## 0. 现状审查结论

| 对象 | 现状 |
|---|---|
| `investment_cases` | id, user_id, provider, provider_id, symbol, name, exchange, currency, asset_class, document_id, created_at；UNIQUE(user_id, provider, provider_id) |
| `decision_records` | decision_type(6) / confidence(smallint 1-5) / reason / evidence / counter_evidence / invalidation / watch_items / recorded_at / snapshot_*（price, price_at, quantity, avg_cost, cost_basis, market_value, unrealized_pnl, pct, 身份五件套） |
| Investment Case 前端 | live 块 → operations → 决策时间线 → 记录决策 Modal（大表单）→ 内嵌 Editor |
| Editor 集成 | document_id 关联，内嵌 Editor + autosave；动态数据不入 content JSON |
| position weight | 前端按组合市值计算（`a.market_value / totalMv`）；可服务端化复用 |

## A. decision_records 可继续复用的字段

| 字段 | 新模型中的角色 |
|---|---|
| `decision_type`（6 值） | **"action after this change"** —— 枚举完全一致（observe/hold/buy/add/reduce/exit），保留列名、语义重定义为动作 |
| `confidence` | **变为 `confidence_after`**（决策/变更后的信心） |
| `recorded_at` + 全部 `snapshot_*` | **Change Record 的冻结现场**（当时 price/holding/cost/P&L/身份）——语义逐字吻合 |
| `id` / `investment_case_id` / CHECK / 索引 | 原样复用 |

## B. 语义不再合适、需要 9000000103 的字段

1. **`confidence` → RENAME `confidence_after`**（值原样保留，零数据迁移）；新增 `confidence_before smallint NULL`。
2. **大表单五文本字段** `reason/evidence/counter_evidence/invalidation/watch_items`：不再适合主流程（高频记录摩擦源）。保留列（不删旧数据），标记 **deprecated**：主流程 UI 不再收集；`invalidation` 归入 Thesis（低频）、`watch_items` 语义并入 `current_biggest_question`。
3. **新增**（change log 语义）：
   - `what_happened text NOT NULL DEFAULT ''`（唯一必填的高频字段）
   - `impact_direction text NULL CHECK ('strengthen','weaken','neutral')`
   - `affected_assumption_id uuid NULL REFERENCES thesis_assumptions(id) ON DELETE SET NULL`
   - `source text NOT NULL DEFAULT 'manual'`（为未来 AI 起草记录预留标记，本阶段不实现 AI）
4. **Current State 四列**（加到 `investment_cases`，均 NULL 起步）：`current_action`（CHECK 同 6 值）、`current_confidence smallint 1-5`、`valuation_view`（CHECK 'cheap','fair','expensive','unclear'）、`current_biggest_question text DEFAULT ''`。
   - Portfolio 自动部分（price/quantity/avg cost/MV/P&L/weight）**不落库**，继续由 `/live` 实时计算；`/live` 新增 `weight_pct`（每 portfolio slice 与聚合值，服务端复用现有市值计算）。

## C. Investment Thesis 存放：独立表（推荐）

`investment_theses` 独立 1:1 表（`investment_case_id UNIQUE FK`）：
- Thesis 是低频独立工件（核心逻辑/证伪/风险），与高频 change log、热 case 行解耦；
- case 行保持轻量（current state 已占 4 列）；
- 为将来「thesis 版本历史」留空间（Phase 2+）。
- 惰性创建：首次保存 thesis 时才 INSERT（无 thesis 的 case 不产生空行）。

## D. Key assumptions 建模：子表 + 稳定 id（可被 change record 引用）

```sql
CREATE TABLE thesis_assumptions (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    investment_case_id uuid NOT NULL REFERENCES investment_cases(id) ON DELETE CASCADE,
    position smallint NOT NULL DEFAULT 0,   -- 1..3 展示序
    text text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_thesis_assumptions_case ON thesis_assumptions (investment_case_id, position);
```
- 每条假设有**稳定 id** → `decision_records.affected_assumption_id` FK 引用（`ON DELETE SET NULL`），Change Record 精确指认"哪条假设被影响"；
- 不用 JSONB 数组：无法 FK、无法被引用、无法部分更新；
- 数量约束 1~3 由 UI 层保证（数据库不加 CHECK 计数，避免过度约束）。

## E. 与既有 Decision Record / snapshot 的兼容

- **零行迁移**：`confidence→confidence_after` 是 RENAME（值保留）；新列全部 NULL/默认。
- 既有行天然成为合法 Change Record：有 action（decision_type）+ confidence_after + 完整 snapshot + recorded_at；`confidence_before` 为 NULL（诚实表示未知）。
- 既有行的五文本字段保留展示（时间线详情展开可见），不回写、不删除。
- 时间线统一渲染：新字段存在时显示 what_happened/impact/assumption；旧行显示 legacy 详情。
- `/decisions` POST 保持向后兼容：`decision_type` 仍为必填 action（与新增可选字段并存）；不破坏现有前端调用。

## F. 最小 UI 改造方案（页面优先级即布局顺序）

```
1. 当前持仓事实  live 块（现有 + weight_pct）
2. 当前判断      Current State 卡：action 下拉 / confidence / valuation view / biggest question —— 内联紧凑表单，改动即 autosave（PATCH /state）
3. 投资逻辑      Thesis 卡（默认折叠，点开就地编辑）：core thesis / assumptions 1-3（增删）/ invalidation / major risks（PUT /thesis）
4. 记录变化      常驻快速记录条（时间线顶部）：what_happened 单行输入 + impact 三键 + assumption 下拉（可空）+ action 下拉（预填）→ Enter 提交（POST /decisions 扩展体）
5. 变化/决策时间线  统一时间线；新记录高亮 what_happened + impact 方向徽标；snapshot 仍标注"当时"
6. Portfolio operations（现有，不动）
7. 补充研究笔记  Editor 默认折叠（弱化入口，标注"补充研究笔记"），承载财报摘录/长分析/资料
```

- 移除「记录决策」大表单 Modal 作为主入口（保留为 legacy 详情视图的只读展示）。
- 新增 API：`PATCH /api/investment-cases/{id}/state`、`GET/PUT /api/investment-cases/{id}/thesis`；`/live` 增 `weight_pct`。

## G. 每日记录摩擦最小化

1. **预填**：记录条 `confidence_before` = 当前 `current_confidence`、`action` = 当前 `current_action` —— 用户只打 what_happened（唯一必填）+ 点 1 下 impact 方向；
2. **Enter 即保存**，无 Modal、无多步表单；30 秒~1 分钟完成一条；
3. impact 三键（strengthen/weaken/neutral）即点即选，assumption 下拉可空（不强制关联）；
4. Current State 改动即时 autosave（无保存按钮）；
5. Thesis 仅在用户主动点击时编辑（低频），平时折叠零干扰。

## H. 为 AI 预留的接口（本阶段不实现 AI）

| 预留 | 用途（Phase 3+） |
|---|---|
| `decision_records.source`（默认 'manual'） | AI 起草的 change record 打 `source='ai'` 标记，待用户确认后落库（同一 POST 端点） |
| `affected_assumption_id` 稳定引用 | AI 可构建 thesis↔证据图："这次变化影响了哪条假设" |
| `confidence_before/after` 差值 | AI 判断用户观点漂移 |
| `current_biggest_question` | AI 聊天锚点："回答我当前最不确定的问题" |
| `document_id` + Editor | AI 生成长文研究资料的落点（不混入结构化字段） |
| 全部结构化数据经既有 GET API 可读 | Phase 3 通过 MCP catalog 暴露给 Agent（Read 为主） |

## 迁移 9000000103（设计，未执行）

```sql
CREATE TABLE investment_theses (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    investment_case_id uuid NOT NULL UNIQUE REFERENCES investment_cases(id) ON DELETE CASCADE,
    core_thesis text NOT NULL DEFAULT '',
    invalidation text NOT NULL DEFAULT '',
    major_risks text NOT NULL DEFAULT '',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE thesis_assumptions ( ...见 D... );
ALTER TABLE investment_cases
    ADD COLUMN current_action text,
    ADD COLUMN current_confidence smallint,
    ADD COLUMN valuation_view text,
    ADD COLUMN current_biggest_question text NOT NULL DEFAULT '',
    ADD CONSTRAINT investment_cases_current_action_check
        CHECK (current_action IN ('observe','hold','buy','add','reduce','exit')),
    ADD CONSTRAINT investment_cases_current_confidence_check
        CHECK (current_confidence BETWEEN 1 AND 5),
    ADD CONSTRAINT investment_cases_valuation_view_check
        CHECK (valuation_view IN ('cheap','fair','expensive','unclear'));
ALTER TABLE decision_records RENAME COLUMN confidence TO confidence_after;
ALTER TABLE decision_records
    ADD COLUMN confidence_before smallint,
    ADD COLUMN what_happened text NOT NULL DEFAULT '',
    ADD COLUMN impact_direction text,
    ADD COLUMN affected_assumption_id uuid REFERENCES thesis_assumptions(id) ON DELETE SET NULL,
    ADD COLUMN source text NOT NULL DEFAULT 'manual',
    ADD CONSTRAINT decision_records_impact_direction_check
        CHECK (impact_direction IN ('strengthen','weaken','neutral')),
    ADD CONSTRAINT decision_records_confidence_before_check
        CHECK (confidence_before BETWEEN 1 AND 5);
```

全部 additive + 无行迁移；编号 9000000103（custom 段，遵守 CUSTOM_CHANGES.md §3）。

## 验收标准（批准后）

- 既有 601118 决策行在 RENAME 后原值保留、时间线正常渲染
- 记录一条 change：只填 what_happened + 点 impact → 30 秒完成；snapshot 冻结正确
- Current State autosave；weight_pct 与 Portfolio 页一致
- Thesis 保存/回读；assumption 被 change record 引用后删除假设 → SET NULL 不报错
- Editor 折叠为补充研究笔记；不用 Editor 时 case 完整可用
- 回归：Position 计算测试、cargo check、npm build 全绿
