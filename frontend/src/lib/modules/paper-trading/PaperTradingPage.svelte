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
  let runs = $state([]);
  let orders = $state([]);
  let detailLoading = $state(false);
  let showForm = $state(false);
  let saving = $state(false);
  let actionBusy = $state(false);
  let endOpen = $state(false);
  let pendingEnd = $state(null);

  const selected = $derived(detail?.strategy ?? strategies.find((s) => s.id === selectedId) ?? null);
  const account = $derived(detail?.account ?? null);
  const activeActions = $derived(selected?.status !== 'ended');

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
      runs = [];
      orders = [];
      return;
    }
    const keep = strategies.some((s) => s.id === selectedId);
    await select(keep ? selectedId : strategies[0].id);
  }

  async function select(id) {
    selectedId = id;
    detailLoading = true;
    error = '';
    try {
      const [d, r, o] = await Promise.all([
        paperTradingApi.detail(id),
        paperTradingApi.runs(id),
        paperTradingApi.orders(id)
      ]);
      detail = d;
      runs = r;
      orders = o;
    } catch (e) {
      error = e.message;
      detail = null;
      runs = [];
      orders = [];
    } finally {
      detailLoading = false;
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
            <h3>{$t('paper.runs')}</h3>
            {#if !runs.length}
              <p class="muted">{$t('paper.no_runs')}</p>
            {:else}
              <ul class="feed">
                {#each runs as r (r.id)}
                  <li>
                    <div class="feed-head">
                      <span class="decision">{r.decision}</span>
                      <span class="muted">{$t('paper.confidence')} {r.confidence}/5</span>
                      <span class="muted when">{when(r.created_at)}</span>
                    </div>
                    {#if r.inference}<p class="inference">{r.inference}</p>{/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </section>

          <section class="block">
            <h3>{$t('paper.orders')}</h3>
            {#if !orders.length}
              <p class="muted">{$t('paper.no_orders')}</p>
            {:else}
              <div class="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>{$t('paper.run_time')}</th>
                      <th>{$t('paper.side')}</th>
                      <th>{$t('paper.asset')}</th>
                      <th class="num">{$t('paper.qty')}</th>
                      <th class="num">{$t('paper.fill')}</th>
                      <th class="num">{$t('paper.fee')}</th>
                      <th class="num">{$t('paper.quote')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each orders as o (o.id)}
                      <tr>
                        <td>{when(o.created_at)}</td>
                        <td><span class="side {o.side}">{o.side}</span></td>
                        <td class="mono">{o.symbol}</td>
                        <td class="num">{qty(o.requested_quantity)}</td>
                        <td class="num">{money(o.fill_price, o.fee_currency)}</td>
                        <td class="num">{money(o.fee, o.fee_currency)}</td>
                        <td class="num muted">
                          {money(o.quote_price_usd, 'USD')} · {when(o.quote_at)}
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
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
  @media (max-width: 820px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
