<script>
  // Research home — "which instruments am I researching, and what do I hold?"
  //
  // Two top-level groups only:
  //   · 持仓中   — cases whose aggregated Portfolio position quantity > 0,
  //                ranked by unrealized P/L % (high → low; unknown % last).
  //   · 未持仓   — everything else, ranked by latest judgement time (new → old).
  //
  // Data rules:
  //   · positions / P&L come from the existing per-case /live aggregation
  //     (Portfolio SSOT) — the same parallel fetch this page already used;
  //     nothing is stored or duplicated here.
  //   · current action / confidence / valuation / biggest question come from
  //     the case-level projections of the LATEST decision record. The list
  //     endpoint does not return that record's `watch_items`, so the "current
  //     watch" line is not shown here (see report) rather than guessed.
  //   · "no judgement" is an inner status hint only — it never creates a group.
  import { onMount } from 'svelte';
  import { t } from '$lib/i18n';
  import {
    investmentCasesApi,
    fmtSignedMoney,
    fmtPct
  } from '$lib/modules/investment-cases/api.js';
  import PageHeader from '$lib/ui/PageHeader.svelte';
  import EmptyState from '$lib/ui/EmptyState.svelte';
  import Skeleton from '$lib/ui/Skeleton.svelte';
  import ErrorText from '$lib/ui/ErrorText.svelte';

  const VALUATIONS = ['cheap', 'fair', 'expensive', 'unclear'];

  let cases = $state([]);
  let live = $state(new Map()); // caseId → live payload (or null when unavailable)
  let loading = $state(true);
  let error = $state('');

  // Latest judgement time, as a sortable number; missing records sort last.
  const tsOf = (c) => {
    const t = c.latest_change_at ? new Date(c.latest_change_at).getTime() : null;
    return Number.isNaN(t) ? null : t;
  };
  const idCmp = (a, b) => (a.c.id < b.c.id ? -1 : a.c.id > b.c.id ? 1 : 0);
  // Stable tie-breaker used inside both groups.
  const byTimeThenId = (a, b) => {
    const ta = a.ts;
    const tb = b.ts;
    if (ta == null && tb == null) return idCmp(a, b);
    if (ta == null) return 1;
    if (tb == null) return -1;
    if (ta !== tb) return tb - ta;
    return idCmp(a, b);
  };

  // Rows enriched with their (nullable) live position.
  const rows = $derived(
    cases.map((c) => {
      const lv = live.get(c.id) ?? null;
      const quantity = lv?.position?.quantity ?? 0;
      return {
        c,
        lv,
        quantity,
        pct: lv?.position?.unrealized_pct ?? null,
        pnl: lv?.position?.unrealized_pnl ?? null,
        ccy: lv?.currency ?? c.currency,
        // A "judgement" exists when the case carries a projection from its
        // latest decision record (action or confidence).
        judged: (c.current_action != null && c.current_action !== '') || c.current_confidence != null,
        ts: tsOf(c)
      };
    })
  );

  const held = $derived(
    rows
      .filter((r) => r.quantity > 0)
      .sort((a, b) => {
        const ap = a.pct;
        const bp = b.pct;
        if (ap != null && bp != null) {
          if (ap !== bp) return bp - ap; // P/L % high → low
        } else if (ap == null && bp == null) {
          /* fall through to time */
        } else {
          return ap == null ? 1 : -1; // unknown % last
        }
        return byTimeThenId(a, b);
      })
  );
  const unheld = $derived(
    rows.filter((r) => r.quantity <= 0).sort(byTimeThenId)
  );

  const total = $derived(cases.length);
  const heldCount = $derived(held.length);
  const unheldCount = $derived(unheld.length);

  const fmtTime = (s) => {
    if (!s) return null;
    const d = new Date(s);
    return Number.isNaN(d.getTime())
      ? null
      : d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  };
  const fmtQty = (q) =>
    new Intl.NumberFormat(undefined, { maximumFractionDigits: 4 }).format(q);

  function load() {
    loading = true;
    error = '';
    investmentCasesApi
      .list()
      .then(async (rows0) => {
        cases = rows0;
        // Live position per case via the existing aggregation — no extra
        // round beyond what the page already did (positions are not part of
        // the list payload itself).
        const entries = await Promise.all(
          rows0.map(async (c) => {
            try {
              return [c.id, await investmentCasesApi.live(c.id)];
            } catch {
              return [c.id, null];
            }
          })
        );
        live = new Map(entries);
      })
      .catch((e) => {
        error = e.message;
      })
      .finally(() => {
        loading = false;
      });
  }
  onMount(load);
</script>

<div class="page">
  <PageHeader title={$t('module.investment-cases.name')} subtitle={$t('investmentCases.list.lede')}>
    {#snippet actions()}
      {#if total > 0}
        <span class="summary">
          {$t('investmentCases.list.summary', { total, held: heldCount, unheld: unheldCount })}
        </span>
      {/if}
    {/snippet}
  </PageHeader>

  <ErrorText error={error} copyable />

  {#if loading && total === 0}
    <div class="cards">
      <Skeleton height="96px" />
      <Skeleton height="96px" />
      <Skeleton height="96px" />
    </div>
  {:else if total === 0}
    <EmptyState icon="target" title={$t('investmentCases.list.empty')} description={$t('investmentCases.list.emptyHint')} />
  {:else}
    <div class="groups">
      {#if heldCount > 0}
        <section class="group">
          <div class="group-head">
            <h2>{$t('investmentCases.list.groupHeld')}</h2>
            <span class="group-meta">{$t('investmentCases.list.groupHeldMeta', { n: heldCount })}</span>
          </div>
          <ol class="cards">
            {#each held as r (r.c.id)}
              {@const c = r.c}
              {@const quantity = r.quantity}
              {@const pct = r.pct}
              {@const pnl = r.pnl}
              {@const ccy = r.ccy}
              {@const judged = r.judged}
              <li>
                <a class="card" class:cn={c.provider === 'china'} href={`/investment-cases/${c.id}`}>
                  <!-- left · identity + current state -->
                  <div class="id-col">
                    <div class="inst">
                      <span class="name">{c.name || c.symbol}</span>
                      {#if c.symbol}<span class="code">{c.symbol}</span>{/if}
                    </div>
                    <div class="tags">
                      {#if judged}
                        {#if c.current_action}
                          <span class="tag act">{$t(`investmentCases.decision.type.${c.current_action}`)}</span>
                        {/if}
                        {#if c.current_confidence != null}
                          <span class="tag">{c.current_confidence}/5</span>
                        {/if}
                        {#if c.valuation_view && VALUATIONS.includes(c.valuation_view)}
                          <span class="tag">{$t(`investmentCases.state.valuation.${c.valuation_view}`)}</span>
                        {/if}
                      {:else}
                        <span class="tag judge-none">{$t('investmentCases.list.noJudgement')}</span>
                      {/if}
                      <span class="tag hold" class:no={quantity <= 0}>
                        {quantity > 0 ? $t('investmentCases.list.holding', { qty: fmtQty(quantity) }) : $t('investmentCases.list.unheld')}
                      </span>
                      {#if quantity > 0 && pct != null}
                        <span class="tag pct" class:up={pct >= 0} class:down={pct < 0}>
                          {pct >= 0 ? '+' : ''}{fmtPct(pct)}
                        </span>
                      {/if}
                    </div>
                  </div>

                  <!-- middle · current research focus -->
                  <div class="focus-col">
                    {#if judged && c.current_biggest_question}
                      <p class="focus-q"><b>{$t('investmentCases.state.question')}</b><span>{c.current_biggest_question}</span></p>
                    {:else if !judged}
                      <p class="focus-empty">{$t('investmentCases.list.noCurrentState')}</p>
                    {/if}
                  </div>

                  <!-- right · P/L, update time and entry -->
                  <div class="meta-col">
                    {#if quantity > 0}
                      <div class="pnl">
                        {#if pnl != null}
                          <span class="amount" class:up={pnl >= 0} class:down={pnl < 0}>
                            {fmtSignedMoney(pnl, ccy)}
                          </span>
                        {/if}
                        {#if pct != null}
                          <span class="pct" class:up={pct >= 0} class:down={pct < 0}>
                            {pct >= 0 ? '+' : ''}{fmtPct(pct)}
                          </span>
                        {/if}
                      </div>
                    {/if}
                    <span class="upd">{fmtTime(c.latest_change_at) ?? $t('investmentCases.list.noRecords')}</span>
                    <span class="open">{$t('investmentCases.list.open')} →</span>
                  </div>
                </a>
              </li>
            {/each}
          </ol>
        </section>
      {/if}

      {#if unheldCount > 0}
        <section class="group">
          <div class="group-head">
            <h2>{$t('investmentCases.list.groupUnheld')}</h2>
            <span class="group-meta">{$t('investmentCases.list.groupUnheldMeta', { n: unheldCount })}</span>
          </div>
          <ol class="cards">
            {#each unheld as r (r.c.id)}
              {@const c = r.c}
              {@const judged = r.judged}
              <li>
                <a class="card" class:cn={c.provider === 'china'} href={`/investment-cases/${c.id}`}>
                  <div class="id-col">
                    <div class="inst">
                      <span class="name">{c.name || c.symbol}</span>
                      {#if c.symbol}<span class="code">{c.symbol}</span>{/if}
                    </div>
                    <div class="tags">
                      {#if judged}
                        {#if c.current_action}
                          <span class="tag act">{$t(`investmentCases.decision.type.${c.current_action}`)}</span>
                        {/if}
                        {#if c.current_confidence != null}
                          <span class="tag">{c.current_confidence}/5</span>
                        {/if}
                        {#if c.valuation_view && VALUATIONS.includes(c.valuation_view)}
                          <span class="tag">{$t(`investmentCases.state.valuation.${c.valuation_view}`)}</span>
                        {/if}
                      {:else}
                        <span class="tag judge-none">{$t('investmentCases.list.noJudgement')}</span>
                      {/if}
                      <span class="tag hold no">{$t('investmentCases.list.unheld')}</span>
                    </div>
                  </div>

                  <div class="focus-col">
                    {#if judged && c.current_biggest_question}
                      <p class="focus-q"><b>{$t('investmentCases.state.question')}</b><span>{c.current_biggest_question}</span></p>
                    {:else if !judged}
                      <p class="focus-empty">{$t('investmentCases.list.noCurrentState')}</p>
                    {/if}
                  </div>

                  <div class="meta-col">
                    <span class="upd">{fmtTime(c.latest_change_at) ?? $t('investmentCases.list.noRecords')}</span>
                    <span class="open">{$t('investmentCases.list.open')} →</span>
                  </div>
                </a>
              </li>
            {/each}
          </ol>
        </section>
      {/if}
    </div>
  {/if}
</div>

<style>
  .page {
    height: 100%;
    overflow-y: auto;
    padding: var(--space-6);
    /* Same centered reading width as the research detail page. */
    width: 100%;
    max-width: 1160px;
    margin: 0 auto;
  }
  .summary {
    color: var(--dim);
    font-size: var(--text-sm);
    white-space: nowrap;
  }

  .groups {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .group-head {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  .group-head h2 {
    margin: 0;
    color: var(--text);
    font-size: var(--text-md);
    font-weight: var(--fw-medium);
  }
  .group-meta {
    color: var(--dim);
    font-size: var(--text-xs);
  }

  .cards {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .card {
    display: grid;
    grid-template-columns: minmax(0, 300px) minmax(0, 1fr) auto;
    gap: var(--space-5);
    align-items: center;
    background: var(--card);
    border: var(--hairline) solid var(--border);
    border-radius: var(--radius);
    padding: var(--space-4) var(--space-5);
    text-decoration: none;
    min-width: 0;
  }
  .card:hover {
    border-color: var(--border-control);
    background: var(--surface-2);
  }
  .card:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  /* left · identity + status */
  .id-col {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .inst {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    min-width: 0;
  }
  .name {
    color: var(--text);
    font-size: var(--text-md);
    font-weight: var(--fw-medium);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .code {
    color: var(--muted);
    font-size: var(--text-sm);
    font-family: var(--mono);
    letter-spacing: 0.02em;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .tag {
    display: inline-flex;
    align-items: center;
    color: var(--muted);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1px 8px;
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .tag.act {
    color: var(--text);
    font-weight: var(--fw-medium);
  }
  .tag.judge-none {
    color: var(--dim);
    border-style: dashed;
  }
  .tag.hold.no {
    color: var(--dim);
  }
  .tag.pct {
    font-variant-numeric: tabular-nums;
  }
  .tag.pct.up {
    color: var(--green);
  }
  .tag.pct.down {
    color: var(--red);
  }
  .cn .tag.pct.up {
    color: var(--red);
  }
  .cn .tag.pct.down {
    color: var(--green);
  }

  /* middle · research focus */
  .focus-col {
    min-width: 0;
    display: flex;
    align-items: center;
  }
  .focus-q {
    margin: 0;
    min-width: 0;
    color: var(--text);
    font-size: var(--text-sm);
    line-height: var(--lh-tight);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .focus-q b {
    color: var(--dim);
    font-weight: 500;
    font-size: var(--text-xs);
  }
  .focus-q span {
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .focus-empty {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-sm);
  }

  /* right · P/L, update time and entry */
  .meta-col {
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--space-1);
    min-width: 150px;
  }
  .pnl {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
  }
  .pnl .amount {
    color: var(--text);
    font-size: var(--text-base);
    font-variant-numeric: tabular-nums;
  }
  .pnl .pct {
    font-size: var(--text-sm);
    font-variant-numeric: tabular-nums;
  }
  .amount.up,
  .pct.up {
    color: var(--green);
  }
  .amount.down,
  .pct.down {
    color: var(--red);
  }
  .cn .amount.up,
  .cn .pct.up {
    color: var(--red);
  }
  .cn .amount.down,
  .cn .pct.down {
    color: var(--green);
  }
  .upd {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .open {
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .card:hover .open {
    color: var(--accent);
  }

  /* tablets: drop the fixed identity width */
  @media (max-width: 980px) {
    .card {
      grid-template-columns: minmax(0, 260px) minmax(0, 1fr);
    }
    .meta-col {
      grid-column: 1 / -1;
      flex-direction: row;
      align-items: center;
      justify-content: space-between;
      flex-wrap: wrap;
      min-width: 0;
    }
  }

  /* phones: single vertical stack, no horizontal overflow */
  @media (max-width: 640px) {
    .page {
      padding: var(--space-4);
    }
    .card {
      grid-template-columns: minmax(0, 1fr);
      gap: var(--space-3);
    }
    .meta-col {
      grid-column: auto;
      align-items: flex-start;
      border-top: var(--hairline) solid var(--border);
      padding-top: var(--space-2);
    }
  }
</style>
