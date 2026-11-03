<script>
  // AI Paper Trading manager. Strategies are created here (human/session only); the MCP
  // agent can then add assets, record runs and place simulated orders inside them. This
  // page only reads the ledger — Portfolio Tracker remains the single source of truth.
  import { onMount } from 'svelte';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import Button from '$lib/ui/Button.svelte';
  import EmptyState from '$lib/ui/EmptyState.svelte';
  import Skeleton from '$lib/ui/Skeleton.svelte';
  import ErrorText from '$lib/ui/ErrorText.svelte';
  import ConfirmModal from '$lib/ui/ConfirmModal.svelte';
  import { t } from '$lib/i18n';
  import StrategyForm from './StrategyForm.svelte';
  import { paperTradingApi, money, qty, pct, when, STATUS_KEY } from './api.js';

  let strategies = $state([]);
  let loading = $state(true);
  let error = $state('');
  let selectedId = $state(null);
  let detail = $state(null);
  // Compact daily timeline, server-paginated. Individual AI cycles load only on expansion.
  const PAGE_SIZE = 10;
  let days = $state([]);
  let totalDays = $state(0);
  let dayPage = $state(0);
  let daysLoading = $state(false);
  let selectedDay = $state(null);
  let dayDetail = $state(null);
  let dayLoading = $state(false);
  let documents = $state([]);
  let openingAssetId = $state(null);
  let detailLoading = $state(false);
  let showForm = $state(false);
  let saving = $state(false);
  let actionBusy = $state(false);
  let endOpen = $state(false);
  let pendingEnd = $state(null);

  const selected = $derived(detail?.strategy ?? strategies.find((s) => s.id === selectedId) ?? null);
  const account = $derived(detail?.account ?? null);
  const activeActions = $derived(selected?.status !== 'ended');
  const pageCount = $derived(Math.ceil(totalDays / PAGE_SIZE));

  onMount(async () => {
    try {
      await reload();
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  });

  async function reload() {
    error = '';
    strategies = await paperTradingApi.list();
    if (!strategies.length) {
      selectedId = null;
      detail = null;
      days = [];
      totalDays = 0;
      documents = [];
      selectedDay = null;
      dayDetail = null;
      return;
    }
    const keep = strategies.some((s) => s.id === selectedId);
    await select(keep ? selectedId : strategies[0].id);
  }

  async function select(id) {
    selectedId = id;
    detailLoading = true;
    error = '';
    selectedDay = null;
    dayDetail = null;
    dayPage = 0;
    try {
      const [d, docs, activity] = await Promise.all([
        paperTradingApi.detail(id),
        paperTradingApi.documents(id),
        paperTradingApi.days(id, PAGE_SIZE, 0)
      ]);
      if (id !== selectedId) return;
      detail = d;
      documents = docs;
      days = activity.days ?? [];
      totalDays = activity.total ?? 0;
    } catch (e) {
      if (id !== selectedId) return;
      error = e.message;
      detail = null;
      days = [];
      totalDays = 0;
      documents = [];
    } finally {
      if (id === selectedId) detailLoading = false;
    }
  }

  async function goToPage(page) {
    if (page < 0 || page >= pageCount || daysLoading || !selectedId) return;
    daysLoading = true;
    selectedDay = null;
    dayDetail = null;
    try {
      const result = await paperTradingApi.days(selectedId, PAGE_SIZE, page * PAGE_SIZE);
      days = result.days ?? [];
      totalDays = result.total ?? 0;
      dayPage = page;
    } catch (e) {
      error = e.message;
    } finally {
      daysLoading = false;
    }
  }

  async function toggleDay(date) {
    if (selectedDay === date) {
      selectedDay = null;
      dayDetail = null;
      return;
    }
    const strategyId = selectedId;
    selectedDay = date;
    dayDetail = null;
    dayLoading = true;
    try {
      const result = await paperTradingApi.day(strategyId, date);
      if (strategyId === selectedId && selectedDay === date) dayDetail = result;
    } catch (e) {
      error = e.message;
    } finally {
      dayLoading = false;
    }
  }

  async function openDocument(assetId) {
    if (!selectedId || openingAssetId) return;
    openingAssetId = assetId;
    try {
      const doc = await paperTradingApi.ensureDocument(selectedId, assetId);
      // The Editor's query parameter selects this exact page, not the first document.
      window.location.assign(`/editor?doc=${encodeURIComponent(doc.id)}`);
    } catch (e) {
      error = e.message;
      openingAssetId = null;
    }
  }

  async function create(payload) {
    saving = true;
    try {
      const strategy = await paperTradingApi.create(payload);
      showForm = false;
      selectedId = strategy.id;
      await reload();
    } finally {
      saving = false;
    }
  }

  async function changeStatus(strategy, status) {
    actionBusy = true;
    error = '';
    try {
      await paperTradingApi.setStatus(strategy.id, status);
      await reload();
    } catch (e) {
      error = e.message;
    } finally {
      actionBusy = false;
    }
  }

  function askEnd(strategy) {
    pendingEnd = strategy;
    endOpen = true;
  }

  async function confirmEnd() {
    const strategy = pendingEnd;
    pendingEnd = null;
    if (strategy) await changeStatus(strategy, 'ended');
  }
</script>

<div class="paper">
  <PageHeader title={$t('paper.title')} subtitle={$t('paper.subtitle')}>
    {#snippet actions()}
      <Button variant="primary" icon="plus" onclick={() => (showForm = true)}>
        {$t('paper.new')}
      </Button>
    {/snippet}
  </PageHeader>

  <ErrorText {error} />

  {#if loading}
    <div class="loading">
      <Skeleton height="4.5rem" />
      <Skeleton height="12rem" />
    </div>
  {:else if !strategies.length}
    <EmptyState icon="sparkles" title={$t('paper.empty.title')} description={$t('paper.empty.desc')}>
      {#snippet action()}
        <Button variant="primary" icon="plus" onclick={() => (showForm = true)}>
          {$t('paper.new')}
        </Button>
      {/snippet}
    </EmptyState>
  {:else}
    <div class="grid">
      <aside class="list" aria-label={$t('paper.strategies')}>
        {#each strategies as s (s.id)}
          <button
            class="card"
            class:active={s.id === selectedId}
            onclick={() => select(s.id)}
            aria-pressed={s.id === selectedId}
          >
            <span class="row">
              <span class="name">{s.name}</span>
              <span class="pill {s.status}">{$t(STATUS_KEY[s.status] ?? s.status)}</span>
            </span>
            <span class="meta">
              {money(s.initial_cash, s.currency)} · {$t('paper.start_date')} {s.start_date}
            </span>
          </button>
        {/each}
      </aside>

      <section class="detail">
        {#if detailLoading}
          <Skeleton rows={8} height="2.5rem" />
        {:else if selected}
          <header class="detail-head">
            <div>
              <h2>{selected.name}</h2>
              <p class="sub">
                {$t('paper.initial_cash')} {money(selected.initial_cash, selected.currency)} ·
                {$t('paper.max_weight')} {pct(selected.max_position_pct)} ·
                {$t('paper.max_order')} {money(selected.max_order_value, selected.currency)}
              </p>
            </div>
            <div class="actions">
              {#if selected.status === 'active'}
                <Button
                  variant="secondary"
                  icon="pause"
                  loading={actionBusy}
                  onclick={() => changeStatus(selected, 'paused')}
                >
                  {$t('paper.pause')}
                </Button>
              {:else if selected.status === 'paused'}
                <Button
                  variant="secondary"
                  icon="play"
                  loading={actionBusy}
                  onclick={() => changeStatus(selected, 'active')}
                >
                  {$t('paper.resume')}
                </Button>
              {/if}
              {#if activeActions}
                <Button
                  variant="danger"
                  icon="stop"
                  loading={actionBusy}
                  onclick={() => askEnd(selected)}
                >
                  {$t('paper.end')}
                </Button>
              {:else}
                <span class="ended-note">{$t('paper.ended_note')}</span>
              {/if}
            </div>
          </header>

          {#if account}
            <div class="tiles">
              <div class="tile">
                <span class="k">{$t('paper.cash')}</span>
                <span class="v">{money(account.cash, account.currency)}</span>
              </div>
              <div class="tile">
                <span class="k">{$t('paper.equity')}</span>
                <span class="v">{money(account.equity, account.currency)}</span>
              </div>
              <div class="tile">
                <span class="k">{$t('paper.market_value')}</span>
                <span class="v">{money(account.market_value, account.currency)}</span>
              </div>
              <div class="tile">
                <span class="k">{$t('paper.start_date')}</span>
                <span class="v">{selected.start_date}</span>
              </div>
            </div>
            {#if !account.market_value_complete}
              <p class="warn">
                {$t('paper.account_incomplete')}
                {#if account.unpriced_holdings?.length}
                  <span class="mono">{account.unpriced_holdings.join(', ')}</span>
                {/if}
              </p>
            {/if}
          {/if}

          <section class="block">
            <h3>{$t('paper.holdings')}</h3>
            {#if !account?.holdings?.length}
              <p class="muted">{$t('paper.no_holdings')}</p>
            {:else}
              <div class="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>{$t('paper.asset')}</th>
                      <th class="num">{$t('paper.qty')}</th>
                      <th class="num">{$t('paper.avg_cost')}</th>
                      <th class="num">{$t('paper.price')}</th>
                      <th class="num">{$t('paper.market_value')}</th>
                      <th class="num">{$t('paper.unrealized')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each account.holdings as h (h.id)}
                      <tr>
                        <td>
                          <span class="mono">{h.symbol}</span>
                          {#if h.name}<span class="muted"> {h.name}</span>{/if}
                          <button
                            class="doc-link"
                            disabled={openingAssetId !== null}
                            title={$t('paper.open_thesis')}
                            onclick={() => openDocument(h.id)}
                          >
                            {openingAssetId === h.id ? '…' : $t('paper.open_thesis')}
                          </button>
                        </td>
                        <td class="num">{qty(h.quantity)}</td>
                        <td class="num">{money(h.avg_cost, account.currency)}</td>
                        <td class="num">{money(h.price, account.currency)}</td>
                        <td class="num">{money(h.market_value, account.currency)}</td>
                        <td class="num" class:pos={h.unrealized > 0} class:neg={h.unrealized < 0}>
                          {money(h.unrealized, account.currency)}
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
          </section>

          <section class="block">
            <header class="day-title">
              <div>
                <h3>{$t('paper.daily_activity')}</h3>
                <p class="muted">{$t('paper.daily_note')}</p>
              </div>
              <span class="muted">{totalDays} {$t('paper.days_total')}</span>
            </header>
            {#if !days.length}
              <p class="muted">{$t('paper.no_runs')}</p>
            {:else}
              <div class="day-list" class:busy={daysLoading}>
                {#each days as day (day.date)}
                  <div class="day-card">
                    <button
                      class="day-button"
                      aria-expanded={selectedDay === day.date}
                      onclick={() => toggleDay(day.date)}
                    >
                      <span class="day-date">{day.date}</span>
                      <span class="muted">{day.order_count} {$t('paper.trades')} · {day.run_count} {$t('paper.analyses')}</span>
                      <span class:pos={day.estimated_pnl > 0} class:neg={day.estimated_pnl < 0} class="day-pnl">
                        {day.estimated_pnl == null ? '—' : money(day.estimated_pnl, account?.currency)}
                      </span>
                      <span class="muted">{selectedDay === day.date ? '−' : '+'}</span>
                    </button>
                    {#if selectedDay === day.date}
                      <div class="day-content">
                        {#if dayLoading}
                          <Skeleton rows={3} height="2rem" />
                        {:else if dayDetail}
                          <h4>{$t('paper.trades')}</h4>
                          {#if !dayDetail.orders?.length}
                            <p class="muted">{$t('paper.no_orders')}</p>
                          {:else}
                            <div class="table-wrap">
                              <table>
                                <thead><tr>
                                  <th>{$t('paper.run_time')}</th>
                                  <th>{$t('paper.side')}</th>
                                  <th>{$t('paper.asset')}</th>
                                  <th class="num">{$t('paper.qty')}</th>
                                  <th class="num">{$t('paper.fill')}</th>
                                  <th class="num">{$t('paper.fee')}</th>
                                </tr></thead>
                                <tbody>
                                  {#each dayDetail.orders as o (o.id)}
                                    <tr>
                                      <td>{when(o.created_at)}</td>
                                      <td><span class="side {o.side}">{o.side}</span></td>
                                      <td class="mono">{o.symbol}</td>
                                      <td class="num">{qty(o.requested_quantity)}</td>
                                      <td class="num">{money(o.fill_price, o.fee_currency)}</td>
                                      <td class="num">{money(o.fee, o.fee_currency)}</td>
                                    </tr>
                                  {/each}
                                </tbody>
                              </table>
                            </div>
                          {/if}
                          <h4>{$t('paper.analyses')}</h4>
                          {#each dayDetail.runs ?? [] as r (r.id)}
                            <details class="run-detail">
                              <summary>
                                <strong class="decision">{r.decision}</strong>
                                <span class="muted">{$t('paper.confidence')} {r.confidence}/5</span>
                                <span class="muted">{when(r.scheduled_at)}</span>
                              </summary>
                              {#if r.inference}<p class="inference">{r.inference}</p>{/if}
                              <p class="muted">{$t('paper.market_facts')}</p>
                              <pre>{JSON.stringify(r.market_facts ?? {}, null, 2)}</pre>
                              <p class="muted">{$t('paper.market_expectations')}</p>
                              <pre>{JSON.stringify(r.market_expectations ?? {}, null, 2)}</pre>
                              <p class="muted">{$t('paper.evidence')}</p>
                              <pre>{JSON.stringify(r.evidence ?? [], null, 2)}</pre>
                              <p class="muted">{$t('paper.risk_assessment')}</p>
                              <pre>{JSON.stringify(r.risk_assessment ?? {}, null, 2)}</pre>
                            </details>
                          {/each}
                        {/if}
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
              <nav class="pagination" aria-label={$t('paper.daily_activity')}>
                <button disabled={dayPage <= 0 || daysLoading} onclick={() => goToPage(dayPage - 1)}>
                  {$t('paper.previous')}
                </button>
                <span>{dayPage + 1} / {pageCount || 1}</span>
                <button disabled={dayPage + 1 >= pageCount || daysLoading} onclick={() => goToPage(dayPage + 1)}>
                  {$t('paper.next')}
                </button>
              </nav>
            {/if}
          </section>
        {:else}
          <p class="muted">{$t('paper.empty.desc')}</p>
        {/if}
      </section>
    </div>
  {/if}

  {#if showForm}
    <StrategyForm bind:open={showForm} {saving} onsubmit={create} />
  {/if}

  <ConfirmModal
    bind:open={endOpen}
    title={$t('paper.end.title')}
    message={$t('paper.end.message')}
    confirmLabel={$t('paper.end')}
    cancelLabel={$t('common.cancel')}
    danger
    onconfirm={confirmEnd}
  />
</div>

<style>
  .paper {
    max-width: 1180px;
    margin: 0 auto;
  }
  .loading {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(240px, 300px) minmax(0, 1fr);
    gap: var(--space-5);
    align-items: start;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    width: 100%;
    text-align: left;
    padding: var(--space-3);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    cursor: pointer;
  }
  .card.active {
    border-color: var(--accent);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .name {
    font-weight: var(--fw-medium);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .pill {
    flex: none;
    font-size: var(--text-xs);
    padding: 1px 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    color: var(--dim);
  }
  .pill.active {
    color: var(--green, #7fb894);
    border-color: currentColor;
  }
  .pill.ended {
    opacity: 0.7;
  }
  .detail {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .detail-head {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  .detail-head h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--fw-medium);
  }
  .sub {
    margin: var(--space-1) 0 0;
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .actions {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    flex-wrap: wrap;
  }
  .ended-note {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: var(--space-2);
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-3);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .k {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .v {
    font-size: var(--text-md);
    font-variant-numeric: tabular-nums;
  }
  .warn {
    margin: var(--space-2) 0 0;
    color: var(--amber, #bfa06f);
    font-size: var(--text-xs);
  }
  .block h3 {
    margin: 0 0 var(--space-2);
    font-size: var(--text-sm);
    font-weight: var(--fw-medium);
    color: var(--dim);
  }
  .table-wrap {
    overflow-x: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
  }
  th,
  td {
    padding: var(--space-2) var(--space-3);
    text-align: left;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }
  th {
    color: var(--dim);
    font-size: var(--text-xs);
    font-weight: var(--fw-regular);
  }
  tbody tr:last-child td {
    border-bottom: none;
  }
  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .mono {
    font-family: var(--font-mono, ui-monospace, monospace);
  }
  .muted {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .pos {
    color: var(--green, #7fb894);
  }
  .neg {
    color: var(--red, #c9776b);
  }
  .feed {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .feed li {
    padding: var(--space-3);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .feed-head {
    display: flex;
    gap: var(--space-3);
    align-items: baseline;
    flex-wrap: wrap;
  }
  .decision {
    font-weight: var(--fw-medium);
    text-transform: uppercase;
    font-size: var(--text-xs);
    letter-spacing: 0.04em;
  }
  .when {
    margin-left: auto;
  }
  .inference {
    margin: var(--space-2) 0 0;
    color: var(--text);
    font-size: var(--text-sm);
    white-space: pre-wrap;
  }
  .side {
    text-transform: uppercase;
    font-size: var(--text-xs);
    letter-spacing: 0.04em;
  }
  .side.buy {
    color: var(--green, #7fb894);
  }
  .side.sell {
    color: var(--red, #c9776b);
  }
  .doc-link {
    margin-left: var(--space-2);
    padding: 2px 5px;
    color: var(--accent);
    border: 0;
    background: none;
    font-size: var(--text-xs);
    cursor: pointer;
  }
  .doc-link:disabled { opacity: .5; cursor: wait; }
  .day-title, .day-button, .pagination {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }
  .day-title { margin-bottom: var(--space-3); }
  .day-title h3 { margin: 0; }
  .day-title p { margin: var(--space-1) 0 0; }
  .day-list { display: flex; flex-direction: column; gap: var(--space-2); }
  .day-list.busy { opacity: .55; }
  .day-card { border: 1px solid var(--border); border-radius: var(--radius); overflow: hidden; }
  .day-button {
    border: 0;
    background: var(--surface);
    color: var(--text);
    padding: var(--space-3);
    width: 100%;
    text-align: left;
    cursor: pointer;
    flex-wrap: wrap;
  }
  .day-date { font-weight: var(--fw-medium); }
  .day-pnl { margin-left: auto; font-variant-numeric: tabular-nums; }
  .day-content { border-top: 1px solid var(--border); padding: var(--space-3); }
  .day-content h4 { margin: var(--space-3) 0 var(--space-2); font-size: var(--text-sm); }
  .run-detail { border-top: 1px solid var(--border); padding: var(--space-2) 0; }
  .run-detail summary { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-3); cursor: pointer; }
  .run-detail pre { max-height: 220px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; font-size: var(--text-xs); }
  .pagination { justify-content: flex-end; margin-top: var(--space-3); }
  .pagination button {
    padding: var(--space-2) var(--space-3); background: var(--surface);
    border: 1px solid var(--border); border-radius: var(--radius);
    color: var(--text); cursor: pointer;
  }
  .pagination button:disabled { opacity: .4; cursor: not-allowed; }
  @media (max-width: 820px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
