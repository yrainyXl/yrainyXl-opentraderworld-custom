# CUSTOM_ARCHITECTURE.md

> **本文档是 OpenTraderWorld 私有分支长期架构的唯一权威 Source of Truth，置于仓库根目录，供所有开发 Agent（DeepSeek Harness / Codex / Claude Code 等）在每次任务开始时阅读。**

> **权威性规则**：
> - 本文件是唯一保存完整架构内容的文件，任何架构性决策只修改本文件。
> - 其他文件（包括 `docs/design/OOTW_LONG_TERM_ARCHITECTURE.md`）只能**引用**本文件，不得保存完整副本。
> - 若引用文件与本文件不一致，一律以本文件为准。

---

# OpenTraderWorld A 股研究系统：长期架构、模块边界与开发约束

> 文档定位：**长期架构基线 / Source of Truth**
>
> 适用对象：后续使用 DeepSeek Harness、Codex、Claude Code 等 Agent 对当前 OpenTraderWorld 私有分支进行开发时，均应先阅读本文件。
>
> 核心要求：**不要把每次需求当作孤立功能实现。所有后续改动都应尽量朝本文定义的长期形态收敛。**
>
> 当前阶段仍以“小步 MVP”为主。本文描述的是目标形态和技术边界，并不代表要求一次性实现全部内容。

---

## 1. 项目目标

当前 OpenTraderWorld 不只是要变成一个“可以看 A 股价格”的工具，也不希望最终变成多个彼此割裂、需要重复录入的模块集合。

长期目标是把它定制成一个：

> **以“发现标的 → 建立研究假设 → 做出决策 → 记录交易/持仓 → 观察结果 → 复盘和学习”为主线的个人投资研究系统。**

系统最终应满足五个特征：

1. **简单方便**：同一事实尽量只录入一次，其他模块自动关联、自动读取。
2. **详细**：行情、持仓、交易、决策、研究、复盘都能保留足够上下文。
3. **科学**：区分“客观事实”“主观判断”“事后结果”，尽量避免事后诸葛亮。
4. **模块职责明确**：每个模块回答不同的问题，不重复建设。
5. **可自动化、可 AI 化**：后续能够利用 OTW 自带 Agent、MCP、Automator，把整理、总结、提醒、复盘等重复工作自动化。

---

# 2. 当前已经完成的基础能力

当前定制版本已经完成以下基础链路：

```text
国内行情源
   │
   ├── 腾讯（主）
   └── 新浪（Fallback）
          │
          ▼
cn-market（Python / FastAPI）
          │
          ▼
China Market
          │
    ┌─────┴─────┐
    ▼           ▼
Watchlist   Portfolio Tracker
```

已经验证：

- `601118 海南橡胶` 可以在 Watchlist 搜索、添加和刷新。
- Watchlist 可以显示：当前价格、24h / 3d / 7d / 30d 变化、30 天趋势、CNY、A 股“涨红跌绿”。
- Portfolio Tracker 可以搜索 A 股、添加 CNY 资产、记录买入/卖出 operation、刷新实时价格、计算成本/持仓/市值/未实现与已实现盈亏，并按 A 股习惯显示颜色。
- `cn-market` 与 OTW 通过 Docker 内部网络通信，行情抓取细节不直接散落在前端。

这是后续所有开发的基础，**不要推翻重做**。

---

# 3. 长期主流程

最终产品主线定义为：

```text
                 ┌──────── Market Data
                 │        行情 / K线 / 指标
                 │
Watchlist ──→ Investment Case ──→ Decision ──→ Portfolio
  发现            研究             判断          执行/持仓
                   │               │              │
                   │               │              ▼
                   │               └────────→ Trading Journal
                   │                              分析/复盘
                   │
                   ├──────── News / 财报 / 宏观
                   │
                   └──────── AI Agent
                             分析 / 挑战 / 总结 / 提醒
```

日常理想流程应尽量接近：

```text
① 发现股票 → 加 Watchlist
② 真正开始研究 → “建立研究”
③ 做出买入/卖出 → Portfolio 只录一次成交事实
④ 有新的判断 → “记录决策”
   → 系统自动保存当时行情、持仓、盈亏
   → 用户只写“为什么”
⑤ 最终卖出 → Portfolio 录卖出
⑥ 其他工作 → Journal 自动分析 → AI 自动整理/挑战 → Automator 自动提醒/周报
```

系统必须尽量避免：

```text
Watchlist 输入一次
Portfolio 再输入一次
Journal 再输入一次
Editor 再手动抄一次
```

---

# 4. 核心设计原则

## 4.1 Single Source of Truth：同一事实只允许一个权威来源

| 数据 | 唯一权威来源 |
|---|---|
| 当前行情 / OHLCV | Market Data / Connector |
| 买入卖出事实 | Portfolio Tracker |
| 当前持仓 / 平均成本 / 盈亏 | Portfolio Tracker 派生 |
| 投资假设 | Investment Case |
| 当时为什么做决定 | Decision Record |
| 策略统计 / 胜率 / Expectancy 等 | Trading Journal |
| 新闻事实 | News / 外部数据源 |
| AI 结论 | 只能作为分析结果，不是基础事实 |

禁止同一笔交易在 Portfolio、Journal、Editor 中各维护一套互不关联的数据。

## 4.2 客观事实与主观判断分离

例如：

```text
客观事实：
2026-08-31
601118
买入 1000 股
成交价 6.49

主观判断：
天然橡胶供给收缩
天气扰动可能带来催化
预计利润改善
```

两者必须分开保存。

## 4.3 实时数据与历史快照分离

Research 页面顶部可以显示实时数据：

```text
当前价格：6.72
当前浮盈：+3.54%
```

但 Decision Record 创建后，其中的数据必须冻结：

```text
2026-09-03 10:35
当时价格：6.31
当时持仓：1000
当时成本：6.49
当时浮盈亏：-2.77%
```

即使以后股价变成 8.00，这条决策记录仍保持原值。

## 4.4 AI 不是交易事实来源

AI 可以总结研究、检查假设、比较前后判断、生成复盘草稿、总结新闻、提醒风险、生成周报、发现自相矛盾。

AI 不应该修改真实成交价格、伪造成交、凭空改变持仓、自动删除历史记录、在没有明确授权时执行敏感写操作，也不能把未经验证的信息写成“客观事实”。

Portfolio Ledger 必须始终可信。

---

# 5. 模块职责边界

## 5.1 Watchlist —— “我现在关注什么？”

负责：快速观察、搜索标的、当前价格、短周期变化、Sparkline、简单备注、价格提醒。

不负责：完整研究、交易记录、组合统计、长篇决策日志。

长期入口：

```text
Watchlist 某标的
       │
       └── [开始研究]
              ↓
        Investment Case
```

## 5.2 Market / Instrument Service —— “这个标的是什么，现在发生了什么？”

这是后续技术上最重要的共享基础层。

长期应统一提供：

```text
Instrument Identity
Symbol Search
Spot Quote
Currency
Exchange
Asset Class
Historical Bars
Provider Metadata
```

不要再出现 Watchlist、Portfolio、Editor 各自实现一遍 A 股搜索。

推荐逐渐形成：

```text
                Market / Instrument Service
                          │
           ┌──────────────┼──────────────┐
           ▼              ▼              ▼
       Watchlist      Portfolio       Research
                          │
                          ▼
                    Historical Data
```

### Instrument 唯一身份

不要只以 `"601118"` 作为全局唯一键。长期推荐使用 `market/exchange + symbol`，或者内部统一 `instrument_id`。

示例：

```text
asset_class: equity
market: CN
exchange: SSE
symbol: 601118
currency: CNY
provider: china
provider_id: 601118
```

这样以后增加港股、美股、ETF、指数、北交所、期货时不会因为代码重复发生冲突。

## 5.3 Portfolio Tracker —— “我实际/模拟持有什么？”

Portfolio 是**交易和持仓事实的唯一来源**。

负责：Buy / Sell operation、日期、数量、成交价格、手续费、币种、当前持仓、平均成本、市值、已实现/未实现盈亏、资产配置、组合价值曲线。

不负责：详细研究逻辑、策略统计、决策长文。

Portfolio 可以同时用于实盘记录、模拟组合、研究仓，但在 UI 上应清楚标记组合用途。

## 5.3.1 AI Paper Trading —— “AI 在独立账户里如何验证策略？”

Paper Trading 是 Portfolio Tracker 上方的策略与执行控制层，不是第二套持仓账本。

- 一个 Paper Trading strategy 对应一个独立 Portfolio。
- `paper_trading_strategies` 保存初始资金、策略/模型元数据、费用和风险限制。
- `paper_trading_runs` 保存每轮 AI 分析、事实/预期/推断、风险判断和服务端冻结的账户快照；无交易也必须留痕。
- `paper_trading_orders` 保存模拟委托意图、幂等键、行情来源时间和对应的 Portfolio operation。
- **成交价格、数量、手续费、当前持仓、成本与盈亏仍以 Portfolio Tracker 为唯一事实来源**；Paper Trading 不复制一份 position ledger。
- 可用现金由初始资金加上 Portfolio operations 的现金流派生，按操作日期使用现有 FX 语义换算，不另存可漂移的现金余额。
- Agent 不得提交成交价格或手续费；服务端从 Shared Market Quote 取价，应用滑点/费用模型，并检查现金、可卖数量、单笔金额和单标的仓位上限；China A-share stocks 遵守 T+1，同日买入数量不可当日卖出。
- Paper Portfolio 只能通过 Paper Trading API 写入；普通 Portfolio 更新/删除/资产操作/成交/导入路径必须拒绝修改模拟账本。
- 模拟资产的计价币种和资产类别在创建后不可变，重复添加标的不得重新解释历史成交金额。策略状态、资产添加、run 与订单写入共用策略行锁；订单内所有账本/FX 读取复用同一事务连接，不能在持锁时再申请连接池连接。
- China 模拟成交使用 Asia/Shanghai 业务日期。当前所有 China 标的保守执行 T+1，直到 Market 层能提供权威结算类型；Agent 自填的 asset_class 不得用于绕过结算约束。
- Investment Case 聚合排除 Paper Portfolio，模拟持仓不能污染个人 Research。
- 该模块永远不连接真实券商或交易所，不产生真实委托。

Paper Trading 的默认费用参数是可配置的模拟假设，不等同于所有市场的真实费率；每轮订单必须保存报价时间和实际 Portfolio operation 关联，以便审计。

## 5.4 Investment Case —— “我为什么研究/持有这个标的？”

这是当前 OTW 最缺少、也是后续最值得新增的业务概念。

它不是完全替换 Editor，而是：

> **Editor 作为富文本编辑能力，Investment Case 作为投资研究业务模型。**

建议数据模型：

```text
investment_cases

id
document_id
instrument_id / provider + provider_id
symbol
name
asset_class
currency
status
created_at
closed_at
```

一个 Investment Case 对应一个标的，例如 `海南橡胶 601118`。

页面结构：

```text
海南橡胶 601118
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

【实时状态】
价格
涨跌
持仓
平均成本
市值
浮盈亏

【当前投资论点】
状态
信心
建立时间

【核心假设】
【催化剂】
【证伪条件】

【Portfolio 交易】
自动加载

【Decision Timeline】
自动加载

【研究正文】
继续使用现有 Editor 富文本
```

动态数据不要硬编码进 Editor 的 rich-text JSON。

## 5.5 Decision Record —— “当时为什么这样判断？”

建议字段：

```text
decision_records

id
investment_case_id
created_at

decision_type
confidence

snapshot_price
snapshot_position
snapshot_avg_cost
snapshot_market_value
snapshot_unrealized_pnl
snapshot_unrealized_pct

reason
evidence
counter_evidence
invalidation
watch_items

linked_operation_id (optional)
```

Decision Type 初期可以只有：

```text
观察
继续持有
买入
加仓
减仓
退出
```

创建决策时自动冻结行情和持仓。

用户人工输入的重点应该只有：

```text
我为什么这么判断？
哪些证据支持？
什么情况说明我错了？
信心多少？
接下来观察什么？
```

## 5.6 Trading Journal —— “长期来看我的交易行为到底怎么样？”

Journal 不应该要求对 Portfolio 已存在的基础交易事实再次手工录入。

长期目标：

```text
Portfolio Operation
        │
        ▼
自动关联 / 派生
        │
        ▼
Trading Journal
```

Journal 主要补充：

```text
Strategy
Signal
Planned Stop
Planned Take Profit
Tags
Execution Quality
Mistakes
Review
```

Journal 主要产出：

```text
Win Rate
Expectancy
Profit Factor
Avg Win / Loss
Max Drawdown
Sharpe / Sortino
R
策略表现
标的表现
Tag 表现
错误成本
```

因此长期职责是：

> Portfolio 负责“交易事实”，Journal 负责“交易统计和行为分析”。

## 5.7 Editor —— 通用内容编辑器，而不是业务事实数据库

Editor 继续负责富文本、图片、表格、文件夹、研究正文、自由笔记。

业务关联不应全部埋在富文本 JSON 中，应通过业务表关联：

```text
Investment Case
      │
      └── document_id
             │
             ▼
          Editor
```

## 5.8 Historical Data / Visualization / Backtest

长期负责 OHLCV 历史数据、K 线、指标、策略回测、参数测试、风险分析。

其输入应尽量来自同一 Market / Instrument Service。

不要为 Historical Data 再写一套独立 China 代码。

## 5.9 News / Economic Calendar

作为 Investment Case 的“外部证据层”。

长期可以支持：

```text
601118 Investment Case
       │
       ├── 相关新闻
       ├── 公司事件
       ├── 行业事件
       └── 宏观事件
```

新闻永远是外部数据，不应直接修改投资论点。

---

# 6. AI Agent / MCP / Automator 长期方向

## 6.1 Agent 定位

AI Agent 应该成为：

> **跨模块理解和分析层，而不是新的数据孤岛。**

长期可设置专门 Persona：

```text
Researcher
Portfolio Manager
Journal Analyst
Thesis Challenger
Weekly Reviewer
```

例如可以问：

```text
总结我为什么买海南橡胶。
我现在的持有理由和建仓时相比发生了什么变化？
当前事实有没有触发我的证伪条件？
这次盈利来自原来的投资逻辑，还是市场整体上涨？
找出我的投资逻辑中互相矛盾的部分。
根据我的 Decision Records，总结过去一个月判断变化。
```

Agent 数据来源：

```text
Investment Case
Decision Records
Portfolio
Journal
Market Data
Historical Data
News
```

## 6.2 MCP 定位

MCP 是：

> **AI 访问 OTW 数据的受控入口。**

后续新增模块和 API 时，要考虑是否需要加入 MCP Catalog。

权限原则：

- Research：Read/Write
- Portfolio：默认 Read，敏感写入需要确认
- Journal：Read/Write
- Market：Read
- 删除：必须明确确认
- Secrets / Network / Data wipe：禁止 AI

**Phase 1 补充（C-016）：外部 Agent 的独立 REST 认证层**

在 MCP 之前先增加一层最小验证能力：`GET /api/agent/ping` 使用独立的
`agent_api_tokens`（绑定 `users.id`、携带 `agent` 主体标签与 `scopes`、支持过期/撤销、
只存 SHA-256）。它只用于验证「外部 AI 能否用独立 Token 访问 OTW API」，不是新的交易
通道，也不复用浏览器 session。

三类凭据必须长期保持隔离，不得互相接受：

```text
浏览器 session   cookie otw_session      表 sessions          仅 Web UI
MCP token        前缀 otw_mcp_            表 mcp_tokens        仅 /api/mcp（固定 Token，兼容本地 Codex）
MCP OAuth token  前缀 otw_oat_/otw_ort_   表 oauth_*           仅 /api/mcp（ChatGPT 等 OAuth 客户端）
Agent API token  前缀 otw_agent_          表 agent_api_tokens  仅 /api/agent/*
```

**Phase 1 补充（C-017）：MCP OAuth 2.1 授权服务器**

OTW 在 `otw-core` 内同时提供 OAuth Authorization Server 与 MCP Resource Server：

- 发现：`/.well-known/oauth-protected-resource`、`/.well-known/oauth-authorization-server`；
- 流程：Authorization Code + 强制 PKCE S256 + consent；DCR 与 host CLI 预注册；
- Token：不透明 256-bit，数据库只存 SHA-256；access 15 分钟，refresh 30 天轮换，
  重放会撤销整族；
- 权限：`mcp:read` 默认只读；可单独请求并经 consent 授权 `mcp:paper-trade`，该 scope 只把 `paper-trading` 模块提升为 `rw`，其他模块仍为 `r`；
  删除、`otw_compute`、普通 Portfolio 写入和真实券商交易始终拒绝；
- 用户：OAuth 绑定真实 `users.id`，但普通 Portfolio / Watchlist 等业务表仍是单用户模型，因此 authorize 只允许
  admin（owner）；**不得宣称普通业务数据已实现多用户隔离**。Paper Trading 是受限例外：strategy 绑定 `user_id` 并唯一关联独立 Portfolio，API 每次按当前用户校验 strategy；MCP 的 paper scope 也只写该模块。若未来要支持普通组合级隔离，仍必须给对应业务表增加 ownership，并覆盖列表、详情、子资源与聚合查询。
- 外部地址：只通过 `OTW_PUBLIC_BASE_URL` 配置，禁止从 Host / X-Forwarded-* 生成安全 URL。
- 固定 `otw_mcp_` Token 保留，作为本地 Codex 兼容路径。

Paper Trading API 已按上述独立 scope 接入 MCP：Agent 只能在独立模拟组合中记录运行和下达模拟订单；Token 的 scope 是唯一授权来源，禁止从请求参数推断用户身份。

不要因为 AI 能访问数据库，就绕过 OTW 原有 API / 权限模型。

## 6.3 Skills

长期建议构建少量、明确的 Skill，而不是一个超大 Prompt：

```text
Pre-Investment Review
Thesis Challenge
Decision Review
Position Review
Post-Trade Review
Weekly Portfolio Review
```

每个 Skill 应包含：

1. 什么时候使用
2. 读取哪些模块
3. 具体步骤
4. 必须检查什么
5. 如何验证
6. 最终输出格式

## 6.4 Automator

Automator 用来消除重复工作。

长期场景：

### 每日收盘检查

```text
刷新持仓
   ↓
读取 Watchlist / Portfolio
   ↓
读取重要新闻
   ↓
AI 对比当前 Investment Case
   ↓
只有出现重大变化才提醒
```

### 每周复盘

```text
Portfolio
+ Decision Records
+ Journal
+ 行情
       ↓
AI
       ↓
生成周度投资复盘
       ↓
写入 Editor / 通知
```

重要原则：

> AI / Automator 可以生成草稿、提醒、分析；交易事实和敏感写操作必须继续受控。

---

# 7. China Market 技术架构长期原则

当前正确方向：

```text
OTW
 │
 └── Stable Market Adapter
           │
           ▼
      cn-market
           │
     ┌─────┴─────┐
     ▼           ▼
   腾讯           新浪
```

## 7.1 Provider 细节全部留在 Python Sidecar

未来这些变化：

```text
腾讯字段变化
新浪字段变化
增加 ETF
增加北交所
增加指数
增加分钟线
加缓存
换第三方数据源
增加 fallback
```

优先只修改：

```text
/opt/apps/cn-market
```

不应该每次重新修改 OTW Rust。

## 7.2 Rust 侧最终应稳定成共享接口

不要长期保留大量：

```rust
if provider == "china" { ... }
```

散落在 Watchlist、Portfolio、Editor、Historical Data、Journal。

长期应逐渐收敛成统一 Market / Instrument abstraction。

模块只请求：

```text
search_instruments()
get_spot()
get_bars()
resolve_currency()
resolve_provider_symbol()
```

而不知道底层是腾讯、Yahoo 还是 CoinGecko。

## 7.3 API Contract 要稳定

`cn-market` 对 OTW 的协议应保持兼容。

Instrument：

```json
{
  "symbol": "601118",
  "name": "海南橡胶",
  "asset_class": "equity",
  "exchange": "SSE",
  "currency": "CNY"
}
```

Quote：

```json
{
  "symbol": "601118",
  "price": 6.72,
  "currency": "CNY",
  "timestamp": "...",
  "source": "tencent"
}
```

Bars：

```json
{
  "symbol": "601118",
  "timeframe": "1d",
  "bars": [
    {
      "ts": 0,
      "open": 0,
      "high": 0,
      "low": 0,
      "close": 0,
      "volume": 0
    }
  ]
}
```

后续内部数据源变化，不应破坏 OTW 消费方。

---

# 8. 开发与构建约束

这是后续 Agent **必须遵守** 的开发规则。

## 8.1 禁止每改一点 Rust 就做 Docker Release Build

错误流程：

```text
改几行
→ docker compose build core
→ 等很久
→ 发现错误
→ 再改
→ 再 release build
```

正确流程：

```text
修改代码
   ↓
cargo check
   ↓
必要时 cargo test
   ↓
继续修
   ↓
全部通过
   ↓
最后一次 release Docker build
```

推荐：

```bash
cargo check -p otw-core
cargo test -p otw-core
```

只有准备部署时才做 Docker release build。

## 8.2 Docker BuildKit 必须保留 Cargo 缓存

Core Dockerfile 必须保证持续复用：

```text
/usr/local/cargo/registry
/usr/local/cargo/git
/build/target
```

推荐：

```dockerfile
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --bin otw-core
```

若 `/build/target` 是 cache mount，最终 binary 必须在同一个 RUN 中复制到普通目录，例如 `/out/otw-core`，再供 runtime stage COPY。

目标：

```text
第一次冷编译：可以慢
后续 Rust 小改：必须显著复用 target cache
```

如果后续修改一两个 Rust 文件依旧重新编译几十/上百个第三方 crate，应先排查缓存，而不是继续等待。

## 8.3 国内网络依赖必须可配置

已遇到过：

```text
gcr.io
static.rust-lang.org
```

在服务器网络中不稳定。

原则：

- 使用已经验证可达的镜像/代理。
- 最好通过 ARG / ENV 配置，不要把临时国内镜像逻辑散落在多个地方。
- Docker runtime 基础镜像与 Rust toolchain 下载源都要可替换。
- 不要因为网络失败反复触发全量构建。

## 8.4 SQLx Migration 变化必须自动触发重编译

OTW 使用：

```rust
sqlx::migrate!("./migrations")
```

Migration 在编译期嵌入 binary。

必须增加/保留：

```rust
// core/otw-store/build.rs
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
```

禁止再通过修改 `lib.rs` 注释这种方式强制 migration 重新嵌入。

## 8.5 Migration 编号不得与 Upstream 冲突

每次新建 migration 前：

1. 检查当前 upstream 最新 migration。
2. 检查本地已经使用的 custom migration。
3. 禁止重复使用 upstream 已存在的 version。
4. 维护一份 custom migration 记录。
5. 在真正启动 Core 让 migration 执行前再次确认冲突。

不要因为当前本地分支旧，就随手创建“下一个编号”。

对于长期 custom migration 编号策略，应单独确定并保持一致。

## 8.6 不做破坏性部署

任何 Agent 修改部署时，默认禁止：

```text
docker compose down -v
删除 postgres volume
清空数据库
重建用户数据
删除已有配置
```

除非用户明确要求。

默认只重建必要服务，并保留 PostgreSQL volume、uploads、user data、settings、connectors。

## 8.7 不修改无关项目

修改 OTW 时：

- 不修改 Rhythm。
- 不修改其他 `/opt/apps` 下项目。
- 不修改宿主 Nginx，除非需求明确涉及。
- 不随意开放新公网端口。

---

# 9. Upstream 兼容策略

当前属于：

> **OTW upstream + 私有定制层**

长期要避免私有 fork 越来越难升级。

## 9.1 China-specific 逻辑尽量集中

优先集中到：

```text
cn-market
market/instrument adapter
少量 provider registry
```

避免十几个 Rust 文件各自 `if china`。

## 9.2 新业务尽量模块化

例如 Investment Case，优先：

```text
research/
investment_cases/
decision_records/
```

而不是在原 Editor、Portfolio、Watchlist 里到处塞大量新逻辑。

原模块只增加必要入口：

```text
Watchlist → 开始研究
Portfolio → 研究
```

## 9.3 每次改动必须留下升级说明

长期建议维护：

```text
CUSTOM_CHANGES.md
```

至少记录：

```text
修改原因
修改文件
数据库 migration
Upstream 潜在冲突点
如何验证
如何回滚
```

## 9.4 Upstream 更新后的固定检查

升级 OTW 前后必须检查：

```text
1. migrations 是否撞号
2. Connector / Market API 是否变化
3. Watchlist API 是否变化
4. Portfolio API 是否变化
5. Editor document schema 是否变化
6. MCP catalog 是否变化
7. Dockerfile 是否变化
8. 前端共享格式函数是否变化
```

## 9.5 官方 Release 是独立 root snapshot，升级必须走 linearized vendor/upstream

官方 G-OTW/OpenTraderWorld 的 release tag（v0.0.1–v0.0.9）是彼此独立的 root snapshot commit，
**不是线性 Git history**（每个 Release 都是无父提交）。

- **禁止**直接执行 `git merge upstream/master` 或 cherry-pick 官方 release root。
- upstream tag / `upstream/master` 只作为**官方 snapshot 来源**，不参与直接合并。
- 后续官方升级使用本地 **linearized `vendor/upstream`** 分支：
  1. 新增一个 synthetic commit：tree 等于新官方 release 的 tree，parent 等于上一版 vendor snapshot；
  2. `custom/main` 再 merge `vendor/upstream`，从而保留正常三方 merge base。
- 不要跳过 vendor 步骤直接合并 release root。

---

# 10. AI 开发 Agent 的工作纪律

DeepSeek / Codex / Claude Code 等后续 Agent 开发时必须遵守：

## 修改前

1. 阅读本文件。
2. 阅读当前实际源码。
3. 查看 `git diff`。
4. 明确本次需求属于哪个模块。
5. 检查是否已经存在可复用能力。
6. 判断是否会破坏长期模块边界。

## 开发中

1. 优先复用。
2. 不重复实现相同行情逻辑。
3. 不重复建立交易事实。
4. 不为了快速通过而硬编码单个股票。
5. 不反复 release build。
6. 小步 `cargo check` / frontend check。
7. Migration 先检查版本。
8. 写操作注意数据兼容和回滚。

## 完成后

必须汇报：

```text
修改文件
数据结构变化
Migration
API 变化
构建结果
测试结果
回滚方式
Upstream 潜在冲突
是否需要更新本文档
```

---

# 11. 路线图

## Phase 0 —— 已完成 / 稳定

```text
China Market
Watchlist A股
Portfolio Tracker A股
CNY
涨红跌绿
cn-market sidecar
```

当前任务是保持稳定，不继续在这里无节制加功能。

## Phase 1 —— Investment Case MVP【下一步】

目标：

> 证明“交易事实只录一次，研究自动围绕已有数据组织”。

### MVP 只实现

1. Editor 文档可以关联一个 Instrument。
2. Portfolio 资产行增加“研究”入口。
3. Watchlist 可选增加“开始研究”入口。
4. 第一次点击创建 Investment Case 和对应 Editor 页面，并自动关联股票。
5. 再次点击直接打开已有页面。
6. 页面顶部自动显示股票名称/代码、当前价格、当前持仓、平均成本、市值、未实现盈亏。
7. 页面自动显示该股票的 Portfolio operations。
8. 增加“记录决策”。
9. Decision Record 自动冻结时间、当前价、持仓、平均成本、市值、盈亏。
10. 用户填写决策类型、判断、理由、证伪条件、信心。

### Phase 1 明确不做

- Journal 自动同步
- AI
- Automator
- News 聚合
- Historical Data
- 财报自动抓取
- 复杂量化
- 自动交易

## Phase 2 —— Portfolio → Journal 自动关联

目标：

```text
Portfolio = 交易事实
Journal = 分析层
```

避免重复录入：

```text
symbol
date
side
quantity
price
fee
currency
```

Journal 只增加：

```text
strategy
signal
tags
planned stop
planned target
execution quality
review
```

## Phase 3 —— Research Agent + MCP

让 Agent 可以读取：

```text
Investment Case
Decision Records
Portfolio
Journal
Market Data
```

第一批能力：

```text
总结投资逻辑
比较前后判断
Thesis Challenge
检查证伪条件
生成复盘草稿
```

先 Read 为主，Write 必须确认。

## Phase 4 —— Automator

第一批有价值的自动化：

```text
每日持仓检查
每周投资复盘
重大价格变化提醒
证伪条件复查提醒
```

避免为了“自动化”而自动化。

## Phase 5 —— Historical Data / Visualization / Backtest

等 Market / Instrument 层稳定后再扩展：

```text
China Market
     ↓
Historical Data
     ↓
Visualization
     ↓
Backtest / Quant Tools
```

确保不会再次建立独立 China 数据实现。

---

# 12. Phase 1 MVP 推荐界面

```text
海南橡胶 601118                         China Market
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

¥6.72     +3.54%

持仓         1000 股
平均成本     ¥6.49
市值         ¥6,720
浮盈亏       +¥230

[记录决策]

━━━━━━━━ Portfolio 操作 ━━━━━━━━━

2026-08-31
买入 1000 @ ¥6.49

━━━━━━━━ 决策时间线 ━━━━━━━━━

2026-09-03 10:35
继续持有 · 信心 3/5

当时：
¥6.31
1000 股
成本 ¥6.49
浮盈 -2.77%

判断：
短期下跌没有破坏供给逻辑……

证伪：
如果……

━━━━━━━━ 研究正文 ━━━━━━━━━

核心假设
……

催化剂
……

风险
……
```

实时区始终更新。

Decision Record 的快照永远不更新。

---

# 13. 科学复盘的最终目标

整个系统最终应该能够回答的不只是：

```text
这笔交易赚了多少钱？
```

而是：

```text
当时为什么买？
当时掌握了哪些信息？
哪些假设后来成立？
哪些假设后来证伪？
什么时候开始出现反向证据？
我有没有忽视已经出现的风险？
最后盈利来自原始逻辑还是其他因素？
相同类型的策略长期胜率是多少？
我的哪些行为模式持续造成损失？
哪些判断其实有稳定优势？
```

只有做到这些，Portfolio、Research、Journal 和 AI 的组合才真正有价值。

---

# 14. 最终架构一句话

长期坚持：

> **Market 提供事实，Portfolio 记录交易事实，Investment Case 维护投资逻辑，Decision Log 保存当时判断，Journal 负责统计分析，AI 负责跨模块理解与挑战，Automator 负责减少重复劳动。**

所有后续功能，都应优先判断：

> “这个功能应该属于哪个模块？它是否会导致重复数据？能不能通过关联已有事实而不是重新输入？”

如果答案不清楚，先设计，再实现。

---

# 15. 对后续 Agent 的直接指令

每次开始 OTW 定制任务时：

```text
请先阅读本文件，把它作为当前私有 OTW 分支的长期架构约束。

本次只实现用户明确要求的最小阶段，不要自行提前实现后续 Phase。

如果当前需求会：
- 导致重复数据源
- 在多个模块重复实现 China Market
- 让 Portfolio / Journal 各维护一套交易事实
- 把业务关联硬塞进 Editor 富文本 JSON
- 破坏 Upstream 可升级性
- 引入新的 migration 冲突
- 导致反复冷启动 Rust release build

请先指出冲突，并提出符合本文方向的更小实现。

修改前先阅读源码和 git diff。
开发阶段使用 cargo check / test。
仅在准备部署时做 release Docker build。
完成后必须给出修改文件、migration、测试、回滚和 Upstream 冲突说明。
```

---

**本文件应长期保留在项目仓库，并随着架构性决策更新。不要把临时 bug 修复、一次性调试日志写进本文件。**
