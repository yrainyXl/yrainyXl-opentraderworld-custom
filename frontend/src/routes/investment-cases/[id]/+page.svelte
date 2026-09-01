<script>
  // Investment Case page (Phase 1 MVP): instrument header, live aggregated
  // position (read from the Portfolio Tracker — no facts duplicated here),
  // operations ledger, decision timeline with frozen snapshots, and the
  // research document embedded through the existing Editor component.
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { t } from '$lib/i18n';
  import { investmentCasesApi, fmtMoney, fmtSignedMoney, fmtPct } from '$lib/modules/investment-cases/api.js';
  import { docsApi } from '$lib/modules/editor/api.js';
  import Editor from '$lib/modules/editor/Editor.svelte';
  import DecisionModal from '$lib/modules/investment-cases/DecisionModal.svelte';
  import Icon from '$lib/ui/Icon.svelte';

  const id = $derived($page.params.id);

  let caseData = $state(null);
  let live = $state(null);
  let decisions = $state([]);
  let loading = $state(true);
  let error = $state(null);

  // Research document (Editor embed + autosave).
  let editorRef = $state(null);
  let doc = $state(null);
  let saveState = $state('idle');
  let saveTimer;

  let decisionOpen = $state(false);

  const isChina = $derived(caseData?.provider === 'china');
  const ccy = $derived(live?.currency ?? caseData?.currency ?? 'USD');

  const fmtDate = (s) =>
    s
      ? new Date(s).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
      : '—';

  async function reload() {
    loading = true;
    error = null;
    try {
      const [c, l, d] = await Promise.all([
        investmentCasesApi.get(id),
        investmentCasesApi.live(id),
        investmentCasesApi.decisions(id)
      ]);
      caseData = c;
      live = l;
      decisions = d;
      if (!doc || doc.id !== c.document_id) {
        doc = await docsApi.get(c.document_id);
        setTimeout(() => editorRef?.setContent(doc.content ?? null), 0);
      }
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }
  onMount(reload);

  function onEditorChange(json) {
    saveState = 'saving';
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      try {
        await docsApi.update(caseData.document_id, { content: json });
        saveState = 'saved';
      } catch {
        saveState = 'error';
      }
    }, 800);
  }

  function onTitleChange() {
    if (!doc || !doc.title) return;
    saveState = 'saving';
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      try {
        await docsApi.update(caseData.document_id, { title: doc.title });
        saveState = 'saved';
      } catch {
        saveState = 'error';
      }
    }, 800);
  }

  async function submitDecision(body) {
    await investmentCasesApi.addDecision(id, body);
    decisionOpen = false;
    decisions = await investmentCasesApi.decisions(id);
  }
</script>

{#if loading}
  <div class="pad">…</div>
{:else if error}
  <div class="pad err">{error}</div>
{:else}
  <div class="case" class:cn={isChina}>
    <header class="head">
      <div class="ident">
        <h1>
          {caseData.symbol}
          {#if caseData.name}<span class="name">{caseData.name}</span>{/if}
        </h1>
        <div class="meta">
          {#if caseData.exchange}<span>{caseData.exchange}</span>{/if}
          <span>{caseData.currency}</span>
          <span class="src">{caseData.provider}</span>
        </div>
      </div>
    </header>

    <!-- Live block: computed on every load from the Portfolio Tracker. -->
    <section class="live card">
      <h2>{$t('investmentCases.live.title')}</h2>
      <div class="grid">
        <div class="stat">
          <span class="lbl">{$t('investmentCases.live.price')}</span>
          <strong class="num">{live.price != null ? fmtMoney(live.price, ccy) : '—'}</strong>
          <span class="sub">{fmtDate(live.price_at)}</span>
        </div>
        <div class="stat">
          <span class="lbl">{$t('investmentCases.live.quantity')}</span>
          <strong class="num">{live.position.quantity}</strong>
        </div>
        <div class="stat">
          <span class="lbl">{$t('investmentCases.live.avgCost')}</span>
          <strong class="num">{live.position.avg_cost != null ? fmtMoney(live.position.avg_cost, ccy) : '—'}</strong>
        </div>
        <div class="stat">
          <span class="lbl">{$t('investmentCases.live.costBasis')}</span>
          <strong class="num">{fmtMoney(live.position.cost_basis, ccy)}</strong>
        </div>
        <div class="stat">
          <span class="lbl">{$t('investmentCases.live.marketValue')}</span>
          <strong class="num">{live.position.market_value != null ? fmtMoney(live.position.market_value, ccy) : '—'}</strong>
        </div>
        <div class="stat">
          <span class="lbl">{$t('investmentCases.live.unrealized')}</span>
          <strong
            class="num"
            class:up={live.position.unrealized_pnl > 0}
            class:down={live.position.unrealized_pnl < 0}
          >
            {live.position.unrealized_pnl != null ? fmtSignedMoney(live.position.unrealized_pnl, ccy) : '—'}
          </strong>
          {#if live.position.unrealized_pct != null}
            <span
              class="sub"
              class:up={live.position.unrealized_pct > 0}
              class:down={live.position.unrealized_pct < 0}
            >
              {fmtPct(live.position.unrealized_pct)}
            </span>
          {/if}
        </div>
      </div>
    </section>

    <!-- Operations: Portfolio Tracker is the SSOT; listed here with portfolio context. -->
    <section class="card">
      <h2>{$t('investmentCases.operations.title')}</h2>
      {#if live.operations.length === 0}
        <p class="dim">{$t('investmentCases.operations.empty')}</p>
      {:else}
        <table class="tbl">
          <thead>
            <tr>
              <th>{$t('investmentCases.operations.date')}</th>
              <th>{$t('investmentCases.operations.side')}</th>
              <th>{$t('investmentCases.operations.quantity')}</th>
              <th>{$t('investmentCases.operations.price')}</th>
              <th>{$t('investmentCases.operations.fee')}</th>
              <th>{$t('investmentCases.operations.portfolio')}</th>
            </tr>
          </thead>
          <tbody>
            {#each live.operations as op (op.asset_id + op.op_date + op.side + op.quantity)}
              <tr>
                <td>{op.op_date}</td>
                <td class:up={op.side === 'buy'} class:down={op.side === 'sell'}>
                  {$t(op.side === 'buy' ? 'investmentCases.operations.buy' : 'investmentCases.operations.sell')}
                </td>
                <td class="num">{op.quantity}</td>
                <td class="num">{fmtMoney(op.price, op.currency)}</td>
                <td class="num">{fmtMoney(op.fee, op.currency)}</td>
                <td>{op.portfolio_name}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>

    <!-- Decision timeline: snapshots are frozen at creation time ("当时"). -->
    <section class="card">
      <div class="sec-head">
        <h2>{$t('investmentCases.decisions.title')}</h2>
        <button class="btn" onclick={() => (decisionOpen = true)}>
          <Icon name="plus" size={13} /> {$t('investmentCases.decisions.record')}
        </button>
      </div>
      {#if decisions.length === 0}
        <p class="dim">{$t('investmentCases.decisions.empty')}</p>
      {:else}
        <ol class="timeline">
          {#each decisions as d (d.id)}
            <li>
              <div class="tl-head">
                <strong>{$t(`investmentCases.decision.type.${d.decision_type}`)}</strong>
                <span class="conf">{$t('investmentCases.decisions.confidence')}: {d.confidence}/5</span>
                <span class="when">{fmtDate(d.recorded_at)}</span>
              </div>
              <div class="snap">
                <span>{$t('investmentCases.decisions.then')}</span>
                <span>{d.snapshot_price != null ? fmtMoney(d.snapshot_price, d.snapshot_currency) : '—'}</span>
                <span>{d.snapshot_quantity} {$t('investmentCases.live.quantity')}</span>
                <span>{$t('investmentCases.decisions.cost')}: {d.snapshot_avg_cost != null ? fmtMoney(d.snapshot_avg_cost, d.snapshot_currency) : '—'}</span>
                <span
                  class:up={d.snapshot_unrealized_pnl > 0}
                  class:down={d.snapshot_unrealized_pnl < 0}
                >
                  {d.snapshot_unrealized_pnl != null ? fmtSignedMoney(d.snapshot_unrealized_pnl, d.snapshot_currency) : '—'}
                </span>
              </div>
              {#if d.reason}<p class="field"><b>{$t('investmentCases.decision.reason')}</b> {d.reason}</p>{/if}
              {#if d.evidence}<p class="field"><b>{$t('investmentCases.decision.evidence')}</b> {d.evidence}</p>{/if}
              {#if d.counter_evidence}<p class="field"><b>{$t('investmentCases.decision.counterEvidence')}</b> {d.counter_evidence}</p>{/if}
              {#if d.invalidation}<p class="field"><b>{$t('investmentCases.decision.invalidation')}</b> {d.invalidation}</p>{/if}
              {#if d.watch_items}<p class="field"><b>{$t('investmentCases.decision.watchItems')}</b> {d.watch_items}</p>{/if}
            </li>
          {/each}
        </ol>
      {/if}
    </section>

    <!-- Research document: existing Editor, autosave to the linked document. -->
    <section class="card research">
      <div class="sec-head">
        <h2>{$t('investmentCases.research.title')}</h2>
        <span class="save-state">
          {#if saveState === 'saving'}{$t('investmentCases.research.saving')}
          {:else if saveState === 'saved'}{$t('investmentCases.research.saved')}
          {:else if saveState === 'error'}{$t('investmentCases.research.saveError')}{/if}
        </span>
      </div>
      <div class="doc-title">
        <input
          bind:value={doc.title}
          oninput={onTitleChange}
          placeholder={$t('investmentCases.research.untitled')}
        />
      </div>
      <div class="editor-wrap">
        <Editor
          bind:this={editorRef}
          content={doc.content ?? null}
          onChange={onEditorChange}
          layout={doc.layout ?? 'normal'}
        />
      </div>
    </section>
  </div>

  <DecisionModal open={decisionOpen} onclose={() => (decisionOpen = false)} onsubmit={submitDecision} />
{/if}

<style>
  .pad {
    padding: var(--space-4);
  }
  .err {
    color: var(--red);
  }
  .case {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-4);
  }
  .head h1 {
    margin: 0;
    font-size: var(--font-xl);
  }
  .name {
    color: var(--text-dim);
    font-weight: normal;
    margin-left: var(--space-2);
  }
  .meta {
    display: flex;
    gap: var(--space-3);
    color: var(--text-dim);
    font-size: var(--font-sm);
    margin-top: var(--space-1);
  }
  .card {
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: var(--space-4);
  }
  .card h2 {
    margin: 0 0 var(--space-3);
    font-size: var(--font-md);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: var(--space-3);
  }
  .stat {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .lbl {
    color: var(--text-dim);
    font-size: var(--font-sm);
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .sub {
    color: var(--text-dim);
    font-size: var(--font-sm);
  }
  .up {
    color: var(--green);
  }
  .down {
    color: var(--red);
  }
  /* China Market convention — 涨红跌绿: same direction classes, flipped palette. */
  .cn .up {
    color: var(--red);
  }
  .cn .down {
    color: var(--green);
  }
  .dim {
    color: var(--text-dim);
  }
  .tbl {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--font-sm);
  }
  .tbl th,
  .tbl td {
    text-align: left;
    padding: var(--space-2);
    border-bottom: 1px solid var(--border);
  }
  .tbl td.num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-3);
  }
  .sec-head h2 {
    margin: 0;
  }
  .timeline {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .timeline li {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--space-3);
  }
  .tl-head {
    display: flex;
    gap: var(--space-3);
    align-items: baseline;
    flex-wrap: wrap;
  }
  .conf,
  .when {
    color: var(--text-dim);
    font-size: var(--font-sm);
  }
  .snap {
    display: flex;
    gap: var(--space-3);
    flex-wrap: wrap;
    background: var(--bg);
    border-radius: var(--radius-sm);
    padding: var(--space-2);
    margin: var(--space-2) 0;
    font-size: var(--font-sm);
    font-variant-numeric: tabular-nums;
  }
  .snap span:first-child {
    color: var(--text-dim);
  }
  .field {
    margin: var(--space-1) 0;
    font-size: var(--font-sm);
  }
  .field b {
    color: var(--text-dim);
    font-weight: 500;
  }
  .save-state {
    color: var(--text-dim);
    font-size: var(--font-sm);
  }
  .doc-title input {
    width: 100%;
    box-sizing: border-box;
    border: none;
    background: transparent;
    color: var(--text);
    font-size: var(--font-lg);
    font-weight: 600;
    padding: var(--space-1) 0;
    margin-bottom: var(--space-2);
  }
  .editor-wrap {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--space-2);
    min-height: 200px;
  }
</style>
