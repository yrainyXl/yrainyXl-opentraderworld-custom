<script>
  // Investment Case detail page — the "research workspace" for one instrument.
  //
  // Layout (data-driven, not decorative):
  //   1. persistent top band   — instrument identity + live Market/Portfolio facts
  //   2. current judgement     — DERIVED from the latest decision record
  //   3. tabs: 动态跟踪 / 研究逻辑
  //
  // 动态跟踪 merges two SOURCE-DISTINCT streams into one reverse-chronological
  // timeline: user judgment records (decision_records) and Portfolio operations
  // (read from the Portfolio SSOT aggregation — never duplicated here).
  //
  // 研究逻辑 reuses the existing low-frequency thesis + the Editor research
  // document. No second body of text, no new tables for either.
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { t } from '$lib/i18n';
  import { investmentCasesApi, fmtMoney, fmtSignedMoney, fmtPct } from '$lib/modules/investment-cases/api.js';
  import { docsApi } from '$lib/modules/editor/api.js';
  import Editor from '$lib/modules/editor/Editor.svelte';
  import Tabs from '$lib/ui/Tabs.svelte';
  import Modal from '$lib/ui/Modal.svelte';
  import EmptyState from '$lib/ui/EmptyState.svelte';

  const id = $derived($page.params.id);

  let loading = $state(true);
  let error = $state(null);
  let caseData = $state(null);
  let live = $state(null);
  let decisions = $state([]);

  const isChina = $derived(caseData?.provider === 'china');
  const ccy = $derived(live?.currency ?? caseData?.currency ?? 'USD');

  // The "current judgement" is never a second store: it is the latest record.
  const latest = $derived(decisions[0] ?? null);
  const marketLabel = $derived(
    caseData?.exchange && caseData.exchange !== ''
      ? caseData.exchange
      : caseData?.provider
        ? caseData.provider.toUpperCase()
        : ''
  );

  const actions = ['observe', 'hold', 'buy', 'add', 'reduce', 'exit'];
  const impacts = ['strengthen', 'weaken', 'neutral'];
  const valuations = ['cheap', 'fair', 'expensive', 'unclear'];

  const fmtDateTime = (s) =>
    s ? new Date(s).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '—';
  const fmtDate = (s) => (s ? new Date(s).toLocaleDateString() : '—');

  // ── Load ────────────────────────────────────────────────────────────────────
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
      syncThesis();
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }
  onMount(reload);

  // ── Tabs ────────────────────────────────────────────────────────────────────
  let tab = $state('activity'); // 动态跟踪 is the default landing for every case
  const tabs = $derived([
    { id: 'activity', label: $t('investmentCases.tabs.activity') },
    { id: 'logic', label: $t('investmentCases.tabs.logic') }
  ]);

  // ── Record this round (drawer-like modal) ───────────────────────────────────
  let recordOpen = $state(false);
  let roundBusy = $state(false);
  let roundErr = $state(null);
  let showExtra = $state(false); // evidence / counter-evidence / invalidation
  let round = $state({
    what: '',
    impact: 'neutral',
    action: 'hold',
    confidence: 3,
    valuation: '',
    question: '',
    watch: '',
    assumptionId: '',
    evidence: '',
    counter: '',
    invalidation: ''
  });

  const assumptionOptions = $derived(
    thesis && Array.isArray(thesis.assumptions) ? thesis.assumptions.filter((a) => a.text.trim()) : []
  );

  function openRound() {
    round = {
      what: '',
      impact: 'neutral',
      // Convenience defaults: the next round usually continues the current one.
      action: caseData?.current_action ?? 'hold',
      confidence: caseData?.current_confidence ?? 3,
      valuation: caseData?.valuation_view ?? '',
      question: caseData?.current_biggest_question ?? '',
      watch: latest?.watch_items ?? '',
      assumptionId: '',
      evidence: '',
      counter: '',
      invalidation: ''
    };
    showExtra = false;
    roundErr = null;
    recordOpen = true;
  }

  async function submitRound() {
    if (!round.what.trim() || roundBusy) return;
    roundBusy = true;
    roundErr = null;
    try {
      await investmentCasesApi.addDecision(id, {
        decision_type: round.action,
        confidence: round.confidence,
        what_happened: round.what.trim(),
        impact_direction: round.impact,
        affected_assumption_id: round.assumptionId || null,
        valuation_view: round.valuation || null,
        biggest_question: round.question.trim(),
        watch_items: round.watch.trim(),
        evidence: round.evidence.trim(),
        counter_evidence: round.counter.trim(),
        invalidation: round.invalidation.trim()
      });
      recordOpen = false;
      // The new record tops the timeline; the judgement band re-derives from it.
      await refreshFeed();
    } catch (e) {
      roundErr = e.message;
    } finally {
      roundBusy = false;
    }
  }

  // Refetch the three sources the page renders. Positions/PnL still come from
  // the same /live aggregation — nothing is ever saved on the case itself.
  async function refreshFeed() {
    try {
      const [c, l, d] = await Promise.all([
        investmentCasesApi.get(id),
        investmentCasesApi.live(id),
        investmentCasesApi.decisions(id)
      ]);
      caseData = c;
      live = l;
      decisions = d;
    } catch {
      /* keep current view; next action retries */
    }
  }

  // ── Thesis (low frequency) ──────────────────────────────────────────────────
  let thesis = $state(null);
  let thesisEdit = $state(false);
  let coreThesis = $state('');
  let invalidation = $state('');
  let majorRisks = $state('');
  let assumptions = $state([
    { position: 1, text: '' },
    { position: 2, text: '' },
    { position: 3, text: '' }
  ]);
  let thesisSaveState = $state('idle');

  async function syncThesis() {
    try {
      const th = await investmentCasesApi.getThesis(id);
      thesis = th;
      if (th?.thesis) {
        coreThesis = th.thesis.core_thesis;
        invalidation = th.thesis.invalidation;
        majorRisks = th.thesis.major_risks;
      }
      if (Array.isArray(th?.assumptions) && th.assumptions.length) {
        for (const a of th.assumptions) {
          const slot = assumptions.find((x) => x.position === a.position);
          if (slot) slot.text = a.text;
        }
      }
    } catch {
      /* thesis stays empty — its section shows the never-saved state */
    }
  }

  async function saveThesis() {
    thesisSaveState = 'saving';
    try {
      const kept = assumptions
        .filter((a) => a.text.trim())
        .map((a) => ({ position: a.position, text: a.text.trim() }));
      const saved = await investmentCasesApi.putThesis(id, {
        core_thesis: coreThesis.trim(),
        invalidation: invalidation.trim(),
        major_risks: majorRisks.trim(),
        assumptions: kept
      });
      thesis = saved;
      thesisEdit = false;
      thesisSaveState = 'saved';
    } catch {
      thesisSaveState = 'error';
    }
  }

  // ── Research notes (Editor, inside the 研究逻辑 tab) ────────────────────────
  let notesDoc = $state(null); // { meta + content } of the case research page
  let notesLoading = $state(false);
  let notesErr = $state(null);
  let saveState = $state('idle');
  let saveTimer;
  let editorRef = $state(null);
  // Notes default to READ state: the article is visible without a big editor
  // toolbar permanently on screen. Editing mounts/keeps the SAME Editor and
  // only toggles its `readOnly` — content and autosave are untouched.
  let notesMode = $state('read'); // 'read' | 'edit'
  const notesEditing = $derived(notesMode === 'edit');

  // True when the saved rich-text body contains any actual text (not just a
  // tree of empty paragraphs). Only drives read-state empty messaging.
  function docHasText(node) {
    if (!node) return false;
    if (node.type === 'text') return !!(node.text ?? '').trim();
    const kids = node.content;
    if (!kids) return false;
    return kids.some(docHasText);
  }
  const hasNotesBody = $derived(notesDoc ? docHasText(notesDoc.content) : false);

  // The research page is created/relinked on demand (never as a GET side
  // effect). The case, its records and thesis are never touched here.
  async function ensureNotes() {
    if (notesDoc || notesLoading) return;
    notesLoading = true;
    notesErr = null;
    try {
      const meta = await investmentCasesApi.ensureResearchDocument(id);
      if (caseData && caseData.document_id !== meta.id) {
        caseData = { ...caseData, document_id: meta.id };
      }
      const d = await docsApi.get(meta.id);
      notesDoc = d;
    } catch (e) {
      notesErr = e.message;
    } finally {
      notesLoading = false;
    }
  }

  // The research body opens as soon as the user lands on 研究逻辑 — no extra
  // click gate. ensureNotes() no-ops when already loaded/loading.
  $effect(() => {
    if (tab === 'logic') ensureNotes();
  });

  function startNotes() {
    notesMode = 'edit'; // empty doc → the Editor opens ready to type
    ensureNotes();
  }
  function toggleNotesMode() {
    notesMode = notesEditing ? 'read' : 'edit';
  }

  function onEditorChange(json) {
    // Keep the in-memory copy current so remounting the tab never regresses to
    // an older saved body; the debounced PATCH below persists it.
    if (notesDoc) notesDoc = { ...notesDoc, content: json };
    saveState = 'saving';
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      try {
        await docsApi.update(notesDoc.id, { content: json });
        saveState = 'saved';
      } catch {
        saveState = 'error';
      }
    }, 800);
  }
  function onTitleChange() {
    if (!notesDoc?.title) return;
    saveState = 'saving';
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      try {
        await docsApi.update(notesDoc.id, { title: notesDoc.title });
        saveState = 'saved';
      } catch {
        saveState = 'error';
      }
    }, 800);
  }

  // ── Merged timeline (动态跟踪) ─────────────────────────────────────────────
  // Each entry keeps its own source shape — this is a view over two existing
  // stores, never a copy of either.
  function timelineItems() {
    const rows = [];
    for (const d of decisions) {
      rows.push({ kind: 'decision', ts: new Date(d.recorded_at).getTime(), d });
    }
    for (const op of live?.operations ?? []) {
      const t = Date.parse(`${op.op_date}T00:00:00Z`);
      rows.push({ kind: 'operation', ts: Number.isNaN(t) ? 0 : t, op });
    }
    rows.sort((a, b) => b.ts - a.ts || (a.kind === 'decision' ? -1 : 1));
    return rows;
  }
  const timeline = $derived(timelineItems());
</script>

{#if loading && !caseData}
  <div class="pad">…</div>
{:else if error && !caseData}
  <div class="pad err">{error}</div>
{:else if caseData}
  <div class="case" class:cn={isChina}>
    <a class="crumb" href="/investment-cases">← {$t('module.investment-cases.name')}</a>

    <!-- 1 · Persistent summary: identity + live Market/Portfolio facts (SSOT). -->
    <header class="band">
      <div class="band-line">
        <div class="ident">
          <h1 class="title">{caseData.name || caseData.symbol}</h1>
          {#if caseData.symbol}<span class="code">{caseData.symbol}</span>{/if}
          {#if marketLabel || ccy}
            <span class="mkts">{marketLabel}{#if marketLabel && ccy} · {/if}{ccy}</span>
          {/if}
        </div>
      </div>

      <dl class="stats">
        <div class="stat">
          <dt>{$t('investmentCases.live.price')}</dt>
          <dd class="num">{live?.price != null ? fmtMoney(live.price, ccy) : '—'}</dd>
          <dd class="sub">{live?.price_at ? fmtDateTime(live.price_at) : ''}</dd>
        </div>
        <div class="stat">
          <dt>{$t('investmentCases.live.quantity')}</dt>
          <dd class="num">{live?.position?.quantity ?? 0}</dd>
        </div>
        <div class="stat">
          <dt>{$t('investmentCases.live.avgCost')}</dt>
          <dd class="num">{live?.position?.avg_cost != null ? fmtMoney(live.position.avg_cost, ccy) : '—'}</dd>
        </div>
        <div class="stat">
          <dt>{$t('investmentCases.live.marketValue')}</dt>
          <dd class="num">{live?.position?.market_value != null ? fmtMoney(live.position.market_value, ccy) : '—'}</dd>
        </div>
        <div class="stat">
          <dt>{$t('investmentCases.live.unrealized')}</dt>
          <dd
            class="num"
            class:up={live?.position?.unrealized_pnl > 0}
            class:down={live?.position?.unrealized_pnl < 0}
          >
            {live?.position?.unrealized_pnl != null ? fmtSignedMoney(live.position.unrealized_pnl, ccy) : '—'}
          </dd>
          {#if live?.position?.unrealized_pct != null}
            <dd
              class="sub num"
              class:up={live.position.unrealized_pct > 0}
              class:down={live.position.unrealized_pct < 0}
            >
              {fmtPct(live.position.unrealized_pct)}
            </dd>
          {/if}
        </div>
      </dl>
    </header>

    <!-- 2 · Current judgement — derived from the latest decision record. -->
    <section class="judge">
      <div class="judge-main">
        {#if latest}
          <div class="judge-chips">
            <span class="chip act">{$t(`investmentCases.decision.type.${latest.decision_type}`)}</span>
            <span class="chip">{latest.confidence}/5</span>
            {#if latest.impact_direction}
              <span class="impact {latest.impact_direction}">{$t(`investmentCases.record.impact.${latest.impact_direction}`)}</span>
            {/if}
          </div>
          {#if latest.biggest_question}
            <p class="judge-q"><b>{$t('investmentCases.state.question')}：</b>{latest.biggest_question}</p>
          {/if}
          {#if latest.watch_items}
            <p class="judge-watch"><b>{$t('investmentCases.current.watch')}：</b>{latest.watch_items}</p>
          {/if}
          <p class="judge-upd">{$t('investmentCases.current.updated')}：{fmtDateTime(latest.recorded_at)}</p>
        {:else}
          <p class="judge-empty">{$t('investmentCases.current.empty')}</p>
        {/if}
      </div>
      <div class="judge-cta">
        <button class="btn primary" onclick={openRound}>
          {decisions.length ? $t('investmentCases.round.record') : $t('investmentCases.round.recordFirst')}
        </button>
      </div>
    </section>

    <!-- 3 · Two top-level tabs -->
    <Tabs {tabs} bind:value={tab} ariaLabel={$t('investmentCases.tabs.aria')} />

    {#if tab === 'activity'}
      <!-- ══ 动态跟踪: judgment records + Portfolio operations, one timeline ══ -->
      <section class="activity">
        <div class="activity-bar">
          <div class="activity-title">
            <h2>{$t('investmentCases.activity.title')}</h2>
            <p class="activity-sub">{$t('investmentCases.activity.subtitle')}</p>
          </div>
          <button class="btn ghost" onclick={() => goto('/portfolios')}>
            {$t('investmentCases.portfolio.logTrade')}
          </button>
        </div>

        {#if timeline.length === 0}
          <EmptyState icon="list" title={$t('investmentCases.activity.noActivity')} description={$t('investmentCases.activity.noActivityHint')}>
            {#snippet action()}
              <button class="btn primary" onclick={openRound}>{$t('investmentCases.round.recordFirst')}</button>
            {/snippet}
          </EmptyState>
        {:else}
          <ol class="timeline">
            {#each timeline as item}
              {#if item.kind === 'decision'}
                {@const d = item.d}
                <li class="entry decision">
                  <div class="entry-head">
                    <span class="tag">{$t('investmentCases.timeline.decision')}</span>
                    <span class="when">{fmtDateTime(d.recorded_at)}</span>
                  </div>

                  <div class="chips">
                    <span class="chip act">{$t(`investmentCases.decision.type.${d.decision_type}`)}</span>
                    {#if d.confidence_before != null && d.confidence_before !== d.confidence}
                      <span class="chip dim">{$t('investmentCases.state.confidence')} {d.confidence_before}/5 → {d.confidence}/5</span>
                    {:else}
                      <span class="chip">{$t('investmentCases.state.confidence')} {d.confidence}/5</span>
                    {/if}
                    {#if d.impact_direction}
                      <span class="impact {d.impact_direction}">{$t(`investmentCases.record.impact.${d.impact_direction}`)}</span>
                    {/if}
                    {#if d.valuation_view}
                      <span class="chip">{$t('investmentCases.state.valuation')}：{$t(`investmentCases.state.valuation.${d.valuation_view}`)}</span>
                    {/if}
                  </div>

                  {#if d.what_happened}
                    <p class="what">{d.what_happened}</p>
                  {/if}

                  {#if d.affected_assumption_text}
                    <p class="field"><b>{$t('investmentCases.record.assumption')}：</b>{d.affected_assumption_text}</p>
                  {/if}
                  {#if d.reason}
                    <p class="field"><b>{$t('investmentCases.decision.reason')}：</b>{d.reason}</p>
                  {/if}
                  {#if d.biggest_question}
                    <p class="field"><b>{$t('investmentCases.state.question')}：</b>{d.biggest_question}</p>
                  {/if}
                  {#if d.watch_items}
                    <p class="field"><b>{$t('investmentCases.current.watch')}：</b>{d.watch_items}</p>
                  {/if}

                  {#if d.evidence}
                    <p class="field"><b>{$t('investmentCases.decision.evidence')}：</b>{d.evidence}</p>
                  {/if}
                  {#if d.counter_evidence}
                    <p class="field"><b>{$t('investmentCases.decision.counterEvidence')}：</b>{d.counter_evidence}</p>
                  {/if}
                  {#if d.invalidation}
                    <p class="field"><b>{$t('investmentCases.decision.invalidation')}：</b>{d.invalidation}</p>
                  {/if}

                  <!-- Frozen at record time by the server; never overwritten. -->
                  <div class="snap">
                    <span><b>{$t('investmentCases.snapshot.price')}</b>{d.snapshot_price != null ? fmtMoney(d.snapshot_price, d.snapshot_currency) : '—'}</span>
                    <span><b>{$t('investmentCases.snapshot.quantity')}</b>{d.snapshot_quantity}</span>
                    <span><b>{$t('investmentCases.snapshot.avgCost')}</b>{d.snapshot_avg_cost != null ? fmtMoney(d.snapshot_avg_cost, d.snapshot_currency) : '—'}</span>
                    <span><b>{$t('investmentCases.snapshot.marketValue')}</b>{d.snapshot_market_value != null ? fmtMoney(d.snapshot_market_value, d.snapshot_currency) : '—'}</span>
                    <span class:up={d.snapshot_unrealized_pnl > 0} class:down={d.snapshot_unrealized_pnl < 0}>
                      <b>{$t('investmentCases.snapshot.pnl')}</b>{d.snapshot_unrealized_pnl != null ? fmtSignedMoney(d.snapshot_unrealized_pnl, d.snapshot_currency) : '—'}
                    </span>
                    {#if d.snapshot_unrealized_pct != null}
                      <span class:up={d.snapshot_unrealized_pct > 0} class:down={d.snapshot_unrealized_pct < 0}>
                        <b>{$t('investmentCases.snapshot.pct')}</b>{fmtPct(d.snapshot_unrealized_pct)}
                      </span>
                    {/if}
                  </div>
                </li>
              {:else}
                {@const op = item.op}
                <li class="entry operation">
                  <div class="entry-head">
                    <span class="tag op">{$t('investmentCases.timeline.operation')}</span>
                    <span class="when">{fmtDate(op.op_date)}</span>
                  </div>
                  <div class="op-line">
                    <span class="side" class:buy={op.side === 'buy'} class:sell={op.side === 'sell'}>
                      {op.side === 'buy' ? $t('investmentCases.operations.buy') : $t('investmentCases.operations.sell')}
                    </span>
                    <span class="qty">{op.quantity}</span>
                    <span class="at">@ {fmtMoney(op.price, op.currency)}</span>
                    {#if op.fee > 0}
                      <span class="fee">{$t('investmentCases.operations.fee')} {fmtMoney(op.fee, op.currency)}</span>
                    {/if}
                    <span class="pf">{op.portfolio_name}</span>
                  </div>
                </li>
              {/if}
            {/each}
          </ol>

          {#if decisions.length === 0}
            <div class="mini-empty">
              <p>{$t('investmentCases.activity.noDecisionsHint')}</p>
              <button class="btn ghost" onclick={openRound}>{$t('investmentCases.round.recordFirst')}</button>
            </div>
          {/if}
          {#if !(live?.operations?.length)}
            <div class="mini-empty">
              <p>{$t('investmentCases.activity.noOperationsHint')}</p>
              <button class="btn ghost" onclick={() => goto('/portfolios')}>{$t('investmentCases.portfolio.logTrade')}</button>
            </div>
          {/if}
        {/if}
      </section>
    {:else}
      <!-- ══ 研究逻辑: thesis (existing structured fields) + Editor body ══ -->
      <section class="logic">
        <div class="panel">
          <div class="sec-head">
            <h2>{$t('investmentCases.thesis.title')}</h2>
            {#if !thesisEdit && thesis?.thesis}
              <button class="btn ghost sm" onclick={() => (thesisEdit = true)}>{$t('investmentCases.thesis.edit')}</button>
            {/if}
          </div>

          {#if !thesisEdit}
            {#if !thesis?.thesis}
              <div class="empty-note">
                <p>{$t('investmentCases.thesis.neverSaved')}</p>
                <button class="btn ghost sm" onclick={() => (thesisEdit = true)}>{$t('investmentCases.thesis.edit')}</button>
              </div>
            {:else}
              <div class="thesis-view">
                {#if thesis.thesis.core_thesis}
                  <p class="field"><b>{$t('investmentCases.thesis.core')}</b></p>
                  <p class="text">{thesis.thesis.core_thesis}</p>
                {/if}
                {#if assumptionOptions.length}
                  <p class="field"><b>{$t('investmentCases.thesis.assumptions')}</b></p>
                  <ol class="assumptions">
                    {#each assumptionOptions as a (a.id)}
                      <li>{a.text}</li>
                    {/each}
                  </ol>
                {/if}
                {#if thesis.thesis.invalidation}
                  <p class="field"><b>{$t('investmentCases.thesis.invalidation')}</b></p>
                  <p class="text">{thesis.thesis.invalidation}</p>
                {/if}
                {#if thesis.thesis.major_risks}
                  <p class="field"><b>{$t('investmentCases.thesis.risks')}</b></p>
                  <p class="text">{thesis.thesis.major_risks}</p>
                {/if}
              </div>
            {/if}
          {:else}
            <div class="thesis-edit">
              <label>
                <span>{$t('investmentCases.thesis.core')}</span>
                <textarea bind:value={coreThesis} rows="4"></textarea>
              </label>
              <div class="assumption-slots">
                <span class="lbl">{$t('investmentCases.thesis.assumptions')}</span>
                {#each assumptions as a (a.position)}
                  <input type="text" bind:value={a.text} placeholder="{$t('investmentCases.thesis.assumptionN')} {a.position}" />
                {/each}
              </div>
              <label>
                <span>{$t('investmentCases.thesis.invalidation')}</span>
                <textarea bind:value={invalidation} rows="2"></textarea>
              </label>
              <label>
                <span>{$t('investmentCases.thesis.risks')}</span>
                <textarea bind:value={majorRisks} rows="2"></textarea>
              </label>
              <div class="actions">
                <button class="btn primary" disabled={thesisSaveState === 'saving'} onclick={saveThesis}>
                  {thesisSaveState === 'saving' ? '…' : $t('investmentCases.thesis.save')}
                </button>
                {#if thesisSaveState === 'saved'}
                  <span class="dim">{$t('investmentCases.thesis.saved')}</span>
                {/if}
                <button class="btn ghost" onclick={() => (thesisEdit = false)}>{$t('common.cancel')}</button>
              </div>
            </div>
          {/if}
        </div>

        <!-- Research body: defaults to a READ view; editing reveals the same Editor. -->
        <div class="panel notes-panel">
          <div class="sec-head">
            <div class="notes-head">
              <h2>{$t('investmentCases.notes.title')}</h2>
              {#if saveState === 'saving'}<span class="dim">{$t('investmentCases.research.saving')}</span>{/if}
              {#if saveState === 'saved'}<span class="dim">{$t('investmentCases.research.saved')}</span>{/if}
              {#if saveState === 'error'}<span class="dim err">{$t('investmentCases.research.saveError')}</span>{/if}
            </div>
            {#if notesDoc}
              {#if notesEditing}
                <button class="btn ghost sm" onclick={toggleNotesMode}>{$t('investmentCases.notes.done')}</button>
              {:else if hasNotesBody}
                <button class="btn ghost sm" onclick={toggleNotesMode}>{$t('investmentCases.notes.edit')}</button>
              {/if}
            {/if}
          </div>

          {#if notesErr}
            <p class="err">{notesErr}</p>
            <button class="btn ghost sm" onclick={ensureNotes}>{$t('investmentCases.research.retry')}</button>
          {:else if !notesDoc}
            <div class="pad dim">…</div>
          {:else if !notesEditing && !hasNotesBody}
            <!-- Nothing written yet: one short prompt + a single entry point. -->
            <div class="empty-note">
              <p>{$t('investmentCases.notes.hint')}</p>
              <button class="btn primary sm" onclick={startNotes}>{$t('investmentCases.notes.start')}</button>
            </div>
          {:else}
            {#if notesEditing}
              <div class="doc-title">
                <input bind:value={notesDoc.title} oninput={onTitleChange} placeholder={$t('investmentCases.research.untitled')} />
              </div>
            {/if}
            <!-- Same Editor for read and edit — toggling `readOnly` never clears
                 or re-creates the document; autosave and loading stay as-is. -->
            <div class="editor-wrap" class:readonly={!notesEditing}>
              <Editor
                bind:this={editorRef}
                readOnly={!notesEditing}
                content={notesDoc.content ?? null}
                onChange={onEditorChange}
                layout={notesDoc.layout ?? 'normal'}
              />
            </div>
          {/if}
        </div>
      </section>
    {/if}
  </div>

  <!-- Modal: 记录本轮判断 -->
  <Modal bind:open={recordOpen} size="md" title={$t('investmentCases.round.title')} onclose={() => (recordOpen = false)}>
    <div class="round-form">
      <label>
        <span>{$t('investmentCases.record.what')}</span>
        <textarea bind:value={round.what} rows="2" required></textarea>
      </label>

      <div class="ctl">
        <span class="lbl">{$t('investmentCases.state.action')}</span>
        <div class="chip-row">
          {#each actions as a}
            <button type="button" class="chip" class:on={round.action === a} onclick={() => (round.action = a)}>
              {$t(`investmentCases.decision.type.${a}`)}
            </button>
          {/each}
        </div>
      </div>

      <div class="ctl">
        <span class="lbl">{$t('investmentCases.state.confidence')}</span>
        <div class="chip-row">
          {#each [1, 2, 3, 4, 5] as n}
            <button type="button" class="chip" class:on={round.confidence === n} onclick={() => (round.confidence = n)}>
              {n}/5
            </button>
          {/each}
        </div>
      </div>

      <div class="ctl">
        <span class="lbl">{$t('investmentCases.round.impactTitle')}</span>
        <div class="chip-row">
          {#each impacts as im}
            <button type="button" class="chip impact-chip {im}" class:on={round.impact === im} onclick={() => (round.impact = im)}>
              {$t(`investmentCases.record.impact.${im}`)}
            </button>
          {/each}
        </div>
      </div>

      <label>
        <span>{$t('investmentCases.state.valuation')}</span>
        <select bind:value={round.valuation}>
          <option value="">—</option>
          {#each valuations as v}
            <option value={v}>{$t(`investmentCases.state.valuation.${v}`)}</option>
          {/each}
        </select>
      </label>

      <label>
        <span>{$t('investmentCases.state.question')}</span>
        <input type="text" bind:value={round.question} placeholder="…" />
      </label>

      <label>
        <span>{$t('investmentCases.round.watch')}</span>
        <input type="text" bind:value={round.watch} placeholder="…" />
      </label>

      {#if assumptionOptions.length}
        <label>
          <span>{$t('investmentCases.record.assumption')}</span>
          <select bind:value={round.assumptionId}>
            <option value="">{$t('investmentCases.record.assumptionNone')}</option>
            {#each assumptionOptions as a (a.id)}
              <option value={a.id}>{a.text}</option>
            {/each}
          </select>
        </label>
      {/if}

      <button type="button" class="extra-toggle" onclick={() => (showExtra = !showExtra)} aria-expanded={showExtra}>
        <span>{$t('investmentCases.round.extra')}</span>
        <span class="fold" aria-hidden="true">{showExtra ? '▾' : '▸'}</span>
      </button>

      {#if showExtra}
        <div class="extra">
          <label>
            <span>{$t('investmentCases.round.evidence')}</span>
            <textarea bind:value={round.evidence} rows="2"></textarea>
          </label>
          <label>
            <span>{$t('investmentCases.round.counterEvidence')}</span>
            <textarea bind:value={round.counter} rows="2"></textarea>
          </label>
          <label>
            <span>{$t('investmentCases.round.invalidation')}</span>
            <textarea bind:value={round.invalidation} rows="2"></textarea>
          </label>
        </div>
      {/if}

      {#if roundErr}<p class="err">{roundErr}</p>{/if}
      <p class="hint">{$t('investmentCases.round.modalHint')}</p>
    </div>

    {#snippet footer()}
      <button class="btn ghost" onclick={() => (recordOpen = false)}>{$t('common.cancel')}</button>
      <button class="btn primary" disabled={roundBusy || !round.what.trim()} onclick={submitRound}>
        {roundBusy ? '…' : $t('investmentCases.round.save')}
      </button>
    {/snippet}
  </Modal>
{/if}

<style>
  .pad {
    padding: var(--space-4);
  }
  .err {
    color: var(--red);
    font-size: var(--font-sm);
  }
  .dim {
    color: var(--text-dim);
  }

  .case {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
    overflow-y: auto;
    /* Ultra-wide screens: keep the page readable and centered instead of
       stretching the whole module context edge to edge. */
    width: 100%;
    max-width: 1160px;
    margin: 0 auto;
  }
  .case > * {
    flex-shrink: 0;
  }
  .crumb {
    align-self: flex-start;
    color: var(--dim);
    text-decoration: none;
    font-size: var(--text-sm);
  }
  .crumb:hover {
    color: var(--accent);
  }

  /* ── 1 · Persistent summary band ── */
  .band {
    background: var(--card);
    border: var(--hairline) solid var(--border);
    border-radius: var(--radius);
    padding: var(--space-3) var(--space-4);
  }
  .band-line {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-3);
    flex-wrap: wrap;
  }
  .ident {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    flex-wrap: wrap;
    min-width: 0;
  }
  .title {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: var(--fw-medium);
    color: var(--text);
  }
  .code {
    color: var(--muted);
    font-size: var(--text-sm);
    font-family: var(--mono);
    letter-spacing: 0.02em;
  }
  .mkts {
    color: var(--dim);
    font-size: var(--text-sm);
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: var(--space-2) var(--space-4);
    margin: var(--space-3) 0 0;
    border-top: var(--hairline) solid var(--border);
    padding-top: var(--space-3);
  }
  .stat dt {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .stat dd {
    margin: 2px 0 0;
    color: var(--text);
    font-size: var(--text-sm);
  }
  .num {
    font-variant-numeric: tabular-nums;
  }
  .stat .sub {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .up {
    color: var(--green);
  }
  .down {
    color: var(--red);
  }
  .cn .up {
    color: var(--red);
  }
  .cn .down {
    color: var(--green);
  }

  /* ── 2 · Current judgement strip (derived, never a second store) ── */
  .judge {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-3);
    background: var(--surface);
    border: var(--hairline) solid var(--border);
    border-radius: var(--radius);
    padding: var(--space-3) var(--space-4);
  }
  .judge-main {
    min-width: 0;
    /* Question/watch lines stay short on very wide screens. */
    flex: 1 1 auto;
    max-width: 76ch;
  }
  .judge-chips {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .judge-q,
  .judge-watch {
    margin: var(--space-2) 0 0;
    color: var(--text);
    font-size: var(--text-sm);
    line-height: var(--lh-tight);
    white-space: pre-wrap;
  }
  .judge-q b,
  .judge-watch b {
    color: var(--dim);
    font-weight: 500;
  }
  .judge-upd {
    margin: var(--space-2) 0 0;
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .judge-empty {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .judge-cta {
    flex: none;
    display: flex;
    align-items: center;
    min-height: 34px;
  }

  /* ── tabs / panels ── */
  .activity,
  .logic {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .activity-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }
  .activity-title {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .activity-sub {
    margin: 0;
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .activity-bar h2,
  .panel h2,
  .sec-head h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--fw-medium);
  }
  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .btn {
    background: transparent;
    border: 1px solid var(--border-control);
    border-radius: var(--radius);
    color: var(--text);
    font-family: inherit;
    font-size: var(--text-sm);
    padding: 7px 14px;
    cursor: pointer;
    white-space: nowrap;
  }
  .btn:hover {
    background: var(--surface-2);
  }
  .btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .btn.primary:hover {
    filter: brightness(1.08);
  }
  .btn.primary:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .btn.ghost {
    background: transparent;
  }
  .btn.sm {
    padding: 4px 10px;
    font-size: var(--text-xs);
  }

  /* timeline */
  .timeline {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .entry {
    background: var(--card);
    border: var(--hairline) solid var(--border);
    border-radius: var(--radius);
    padding: var(--space-3) var(--space-4);
  }
  .entry-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    margin-bottom: var(--space-2);
  }
  .tag {
    font-size: var(--text-xs);
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--dim);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1px 8px;
  }
  .tag.op {
    color: var(--accent);
    border-color: var(--accent);
  }
  .when {
    color: var(--dim);
    font-size: var(--text-sm);
    margin-left: auto;
  }
  .chips {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 1px 8px;
    font-size: var(--text-sm);
    color: var(--muted);
  }
  .chip.act {
    color: var(--text);
    font-weight: var(--fw-medium);
  }
  .chip.dim {
    color: var(--dim);
  }
  .impact {
    display: inline-flex;
    align-items: center;
    font-size: var(--text-sm);
    border-radius: var(--radius);
    padding: 1px 8px;
    border: 1px solid var(--border);
    color: var(--text-dim);
  }
  .impact.strengthen {
    color: var(--green);
    border-color: var(--green);
  }
  .cn .impact.strengthen {
    color: var(--red);
    border-color: var(--red);
  }
  .impact.weaken {
    color: var(--red);
    border-color: var(--red);
  }
  .cn .impact.weaken {
    color: var(--green);
    border-color: var(--green);
  }
  .impact.neutral {
    color: var(--text-dim);
  }

  .what {
    margin: var(--space-2) 0;
    color: var(--text);
    font-weight: 600;
    white-space: pre-wrap;
    line-height: var(--lh-base);
    /* readable measure on very wide screens */
    max-width: 76ch;
  }
  .field {
    margin: var(--space-1) 0;
    font-size: var(--text-sm);
    white-space: pre-wrap;
    color: var(--text);
    line-height: var(--lh-base);
  }
  .field b {
    color: var(--dim);
    font-weight: 500;
  }

  .snap {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
    gap: var(--space-2);
    background: var(--bg);
    border: var(--hairline) solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--space-2) var(--space-3);
    margin-top: var(--space-3);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
  }
  .snap span {
    display: flex;
    flex-direction: column;
    gap: 2px;
    color: var(--text);
  }
  .snap b {
    color: var(--dim);
    font-weight: 500;
    font-size: var(--text-xs);
  }

  /* operation entries */
  .op-line {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    flex-wrap: wrap;
    font-size: var(--text-sm);
    color: var(--text);
  }
  .side {
    font-weight: var(--fw-medium);
    border-radius: var(--radius);
    border: 1px solid var(--border);
    padding: 0 8px;
  }
  .side.buy {
    color: var(--green);
    border-color: var(--green);
  }
  .cn .side.buy {
    color: var(--red);
    border-color: var(--red);
  }
  .side.sell {
    color: var(--red);
    border-color: var(--red);
  }
  .cn .side.sell {
    color: var(--green);
    border-color: var(--green);
  }
  .qty {
    font-variant-numeric: tabular-nums;
  }
  .fee,
  .pf {
    color: var(--dim);
    font-size: var(--text-xs);
  }

  .mini-empty {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    border: var(--hairline) dashed var(--border);
    border-radius: var(--radius);
    padding: var(--space-3) var(--space-4);
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .mini-empty p {
    margin: 0;
  }

  /* research logic panels */
  .panel {
    background: var(--card);
    border: var(--hairline) solid var(--border);
    border-radius: var(--radius);
    padding: var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .empty-note {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }
  .empty-note p {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-sm);
  }
  .thesis-view .text {
    margin: var(--space-1) 0 var(--space-3);
    color: var(--text);
    font-size: var(--text-sm);
    white-space: pre-wrap;
    line-height: var(--lh-base);
  }
  .thesis-view .field {
    margin: var(--space-2) 0 0;
  }
  .assumptions {
    margin: var(--space-1) 0 0;
    padding-left: var(--space-4);
    font-size: var(--text-sm);
  }
  .thesis-edit {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .thesis-edit label,
  .round-form label {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: var(--text-sm);
  }
  .thesis-edit label span,
  .round-form label span,
  .ctl .lbl {
    color: var(--dim);
    font-size: var(--text-sm);
  }
  .assumption-slots {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }

  .notes-panel {
    gap: var(--space-2);
  }
  .notes-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
    min-width: 0;
  }
  .doc-title input {
    width: 100%;
    box-sizing: border-box;
    border: none;
    background: transparent;
    color: var(--text);
    font-size: var(--text-lg);
    font-weight: 600;
    padding: var(--space-1) 0;
    margin-bottom: var(--space-1);
  }
  .editor-wrap {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--space-2);
    min-height: 0;
    /* Bounded so the Editor's own scroll region (and its sticky Document
       Outline) is the single scroll container for the notes while editing. */
    height: clamp(420px, 58vh, 720px);
  }
  /* Read view: the box grows with the document — no tall empty pane. */
  .editor-wrap.readonly {
    height: auto;
  }

  /* round modal form */
  .round-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .round-form textarea,
  .round-form input,
  .round-form select {
    width: 100%;
    box-sizing: border-box;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--space-2);
    font: inherit;
    font-size: var(--text-sm);
  }
  .ctl {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .chip-row {
    display: flex;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .chip-row .chip {
    padding: 4px 10px;
    cursor: pointer;
    background: transparent;
    border: 1px solid var(--border-control);
    color: var(--muted);
  }
  .chip-row .chip:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  .chip-row .chip.on {
    color: var(--text);
    border-color: var(--accent);
    background: var(--surface-2);
    font-weight: var(--fw-medium);
  }
  .chip-row .impact-chip.strengthen.on {
    color: var(--green);
    border-color: var(--green);
  }
  .chip-row .impact-chip.weaken.on {
    color: var(--red);
    border-color: var(--red);
  }
  .extra-toggle {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    background: transparent;
    border: none;
    color: var(--muted);
    font: inherit;
    font-size: var(--text-sm);
    cursor: pointer;
    padding: 0;
    width: max-content;
  }
  .extra-toggle:hover {
    color: var(--text);
  }
  .extra {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    border-top: var(--hairline) solid var(--border);
    padding-top: var(--space-3);
  }
  .round-form .hint {
    margin: 0;
    color: var(--dim);
    font-size: var(--text-xs);
  }

  /* ── mobile: summary becomes two columns, controls keep usable sizes ── */
  @media (max-width: 640px) {
    .stats {
      grid-template-columns: repeat(2, 1fr);
    }
    .judge {
      flex-direction: column;
    }
    .judge-main {
      max-width: none;
    }
    .judge-cta {
      width: 100%;
    }
    .judge-cta .btn {
      width: 100%;
    }
    .activity-bar {
      align-items: flex-start;
      flex-direction: column;
    }
    .activity-bar .btn {
      width: 100%;
    }
    .editor-wrap {
      height: 60vh;
    }
    .editor-wrap.readonly {
      height: auto;
    }
  }
</style>
