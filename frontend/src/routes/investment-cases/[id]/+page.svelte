<script>
  // Investment Case page — Phase 1.5 three-layer model:
  //   1. live position facts (Portfolio SSOT)  2. current state
  //   3. investment thesis (low frequency)     4. quick change record
  //   5. change/decision timeline              6. portfolio operations
  //   7. supplementary research notes (Editor, collapsed — not the main flow)
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { t } from '$lib/i18n';
  import { investmentCasesApi, fmtMoney, fmtSignedMoney, fmtPct } from '$lib/modules/investment-cases/api.js';
  import { docsApi } from '$lib/modules/editor/api.js';
  import Editor from '$lib/modules/editor/Editor.svelte';

  const id = $derived($page.params.id);

  let caseData = $state(null);
  let live = $state(null);
  let decisions = $state([]);
  let thesis = $state(null); // {thesis, assumptions}
  let loading = $state(true);
  let error = $state(null);

  const isChina = $derived(caseData?.provider === 'china');
  const ccy = $derived(live?.currency ?? caseData?.currency ?? 'USD');
  const actions = ['observe', 'hold', 'buy', 'add', 'reduce', 'exit'];
  const impacts = ['strengthen', 'weaken', 'neutral'];
  const valuations = ['cheap', 'fair', 'expensive', 'unclear'];

  const fmtDate = (s) =>
    s ? new Date(s).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '—';

  // ── Research notes (Editor) — collapsed, autosave like before.
  let notesOpen = $state(false);
  let editorRef = $state(null);
  let doc = $state(null);
  let saveState = $state('idle');
  let saveTimer;

  // ── Current state (standalone autosave: valuation + biggest question).
  let valuationView = $state(null);
  let biggestQuestion = $state('');
  let stateTimer;

  // ── Thesis (edit in place when opened).
  let thesisOpen = $state(false);
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

  // ── Quick change record.
  let whatHappened = $state('');
  let impactDirection = $state('neutral');
  let recordBusy = $state(false);
  let recordErr = $state(null);
  let actionExpand = $state(false);
  let actionAfter = $state('hold');
  let confidenceAfter = $state(3);
  let selectedAssumptionText = $state('');

  const assumptionOptions = $derived(
    thesis && Array.isArray(thesis.assumptions) ? thesis.assumptions.filter((a) => a.text.trim()) : []
  );

  async function reload() {
    loading = true;
    error = null;
    try {
      const [c, l, d, th] = await Promise.all([
        investmentCasesApi.get(id),
        investmentCasesApi.live(id),
        investmentCasesApi.decisions(id),
        investmentCasesApi.getThesis(id)
      ]);
      caseData = c;
      live = l;
      decisions = d;
      syncStateFromCase(c);
      syncThesisFromApi(th);
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

  function syncStateFromCase(c) {
    valuationView = c.valuation_view ?? null;
    biggestQuestion = c.current_biggest_question ?? '';
    actionAfter = c.current_action ?? 'hold';
    confidenceAfter = c.current_confidence ?? 3;
  }

  function syncThesisFromApi(th) {
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
  }

  // ── Current state autosave (valuation / question only — action/confidence
  //    only change through recording a change).
  function scheduleStateSave() {
    clearTimeout(stateTimer);
    stateTimer = setTimeout(async () => {
      try {
        const c = await investmentCasesApi.patchState(id, {
          valuation_view: valuationView,
          current_biggest_question: biggestQuestion
        });
        caseData = c;
      } catch {
        /* keep local edits; next change retries */
      }
    }, 700);
  }

  // ── Thesis save (upsert; assumptions keep their id when text is edited in place).
  async function saveThesis() {
    thesisSaveState = 'saving';
    recordErr = null;
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
    } catch (e) {
      thesisSaveState = 'error';
      recordErr = e.message;
    }
  }

  // ── Quick change record.
  async function submitChange() {
    if (!whatHappened.trim()) return;
    recordBusy = true;
    recordErr = null;
    try {
      const body = {
        decision_type: actionAfter,
        confidence: confidenceAfter,
        what_happened: whatHappened.trim(),
        impact_direction: impactDirection
      };
      const matched =
        selectedAssumptionText && assumptionOptions.find((a) => a.text === selectedAssumptionText);
      if (matched) body.affected_assumption_id = matched.id;
      await investmentCasesApi.addDecision(id, body);
      whatHappened = '';
      selectedAssumptionText = '';
      actionExpand = false;
      const [c, d] = await Promise.all([
        investmentCasesApi.get(id),
        investmentCasesApi.decisions(id)
      ]);
      caseData = c; // current_action / current_confidence projection updated
      syncStateFromCase(c);
      decisions = d;
    } catch (e) {
      recordErr = e.message;
    } finally {
      recordBusy = false;
    }
  }

  // ── Research notes editor handlers.
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
    if (!doc?.title) return;
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

    <!-- 1. Live position facts (computed from Portfolio SSOT). -->
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
            <span class="sub" class:up={live.position.unrealized_pct > 0} class:down={live.position.unrealized_pct < 0}>
              {fmtPct(live.position.unrealized_pct)}
            </span>
          {/if}
        </div>
      </div>
    </section>

    <!-- 2. Current state: action/confidence project the LATEST change record;
         valuation + biggest question autosave independently. -->
    <section class="card">
      <h2>{$t('investmentCases.state.title')}</h2>
      <div class="state-row">
        <div class="state-item">
          <span class="lbl">{$t('investmentCases.state.action')}</span>
          <strong>{caseData.current_action ? $t(`investmentCases.decision.type.${caseData.current_action}`) : '—'}</strong>
        </div>
        <div class="state-item">
          <span class="lbl">{$t('investmentCases.state.confidence')}</span>
          <strong>{caseData.current_confidence != null ? `${caseData.current_confidence}/5` : '—'}</strong>
        </div>
        <p class="hint">{$t('investmentCases.state.viaChange')}</p>
      </div>
      <div class="state-edit">
        <label>
          <span>{$t('investmentCases.state.valuation')}</span>
          <select bind:value={valuationView} onchange={scheduleStateSave}>
            <option value={null}>—</option>
            {#each valuations as v}
              <option value={v}>{$t(`investmentCases.state.valuation.${v}`)}</option>
            {/each}
          </select>
        </label>
        <label class="grow">
          <span>{$t('investmentCases.state.question')}</span>
          <input
            type="text"
            bind:value={biggestQuestion}
            oninput={scheduleStateSave}
            placeholder="…"
          />
        </label>
      </div>
    </section>

    <!-- 3. Investment thesis (low frequency; collapsed; edit in place). -->
    <section class="card">
      <div
        class="sec-head clickable"
        role="button"
        tabindex="0"
        onclick={() => (thesisOpen = !thesisOpen)}
      >
        <h2>{$t('investmentCases.thesis.title')}</h2>
        <span class="fold">{thesisOpen ? '▾' : '▸'}</span>
      </div>
      {#if thesisOpen}
        {#if !thesisEdit}
          <div class="thesis-view">
            {#if !thesis?.thesis && (!assumptionOptions.length || thesis == null)}
              <p class="dim">{$t('investmentCases.thesis.neverSaved')}</p>
            {:else}
              {#if thesis?.thesis?.core_thesis}
                <p class="field"><b>{$t('investmentCases.thesis.core')}</b><br />{thesis.thesis.core_thesis}</p>
              {/if}
              {#if assumptionOptions.length}
                <p class="field"><b>{$t('investmentCases.thesis.assumptions')}</b></p>
                <ol class="assumptions">
                  {#each assumptionOptions as a (a.id)}
                    <li>{a.text}</li>
                  {/each}
                </ol>
              {/if}
              {#if thesis?.thesis?.invalidation}
                <p class="field"><b>{$t('investmentCases.thesis.invalidation')}</b><br />{thesis.thesis.invalidation}</p>
              {/if}
              {#if thesis?.thesis?.major_risks}
                <p class="field"><b>{$t('investmentCases.thesis.risks')}</b><br />{thesis.thesis.major_risks}</p>
              {/if}
            {/if}
            <div class="actions">
              <button class="btn ghost" onclick={() => (thesisEdit = true)}>{$t('investmentCases.thesis.edit')}</button>
            </div>
          </div>
        {:else}
          <div class="thesis-edit">
            <label>
              <span>{$t('investmentCases.thesis.core')}</span>
              <textarea bind:value={coreThesis} rows="3"></textarea>
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
            {#if recordErr}<p class="err">{recordErr}</p>{/if}
            <div class="actions">
              <button class="btn" disabled={thesisSaveState === 'saving'} onclick={saveThesis}>
                {thesisSaveState === 'saving' ? '…' : $t('investmentCases.thesis.save')}
              </button>
              {#if thesisSaveState === 'saved'}<span class="dim">{$t('investmentCases.thesis.saved')}</span>{/if}
              <button class="btn ghost" onclick={() => (thesisEdit = false)}>{$t('common.cancel')}</button>
            </div>
          </div>
        {/if}
      {/if}
    </section>

    <!-- 4. Quick change record (the main high-frequency interaction). -->
    <section class="card quick">
      <h2>{$t('investmentCases.record.title')}</h2>
      <div class="quick-bar">
        <input
          class="what"
          type="text"
          bind:value={whatHappened}
          onkeydown={(e) => e.key === 'Enter' && submitChange()}
          placeholder={$t('investmentCases.record.what')}
        />
        <div class="impacts">
          {#each impacts as imp}
            <button class="chip" class:on={impactDirection === imp} onclick={() => (impactDirection = imp)}>
              {$t(`investmentCases.record.impact.${imp}`)}
            </button>
          {/each}
        </div>
        <div class="quick-extra">
          {#if assumptionOptions.length}
            <select bind:value={selectedAssumptionText} class="assumption">
              <option value="">{$t('investmentCases.record.assumptionNone')}</option>
              {#each assumptionOptions as a (a.id)}
                <option value={a.text}>{a.text}</option>
              {/each}
            </select>
          {/if}
          {#if !actionExpand}
            <span class="cur">
              {$t(`investmentCases.decision.type.${actionAfter}`)} · {$t('investmentCases.state.confidence')} {confidenceAfter}/5
            </span>
            <button class="link" onclick={() => (actionExpand = true)}>{$t('investmentCases.record.modify')}</button>
          {:else}
            <label>
              <span>{$t('investmentCases.record.action')}</span>
              <select bind:value={actionAfter}>
                {#each actions as act}
                  <option value={act}>{$t(`investmentCases.decision.type.${act}`)}</option>
                {/each}
              </select>
            </label>
            <label>
              <span>{$t('investmentCases.record.confidenceAfter')}</span>
              <select bind:value={confidenceAfter}>
                {#each [1, 2, 3, 4, 5] as c}
                  <option value={c}>{c}/5</option>
                {/each}
              </select>
            </label>
          {/if}
        </div>
        {#if recordErr}<p class="err">{recordErr}</p>{/if}
        <div class="actions">
          <button class="btn" disabled={recordBusy || !whatHappened.trim()} onclick={submitChange}>
            {recordBusy ? '…' : $t('investmentCases.record.save')}
          </button>
        </div>
      </div>
    </section>

    <!-- 5. Change / decision timeline (dual render). -->
    <section class="card">
      <h2>{$t('investmentCases.decisions.title')}</h2>
      {#if decisions.length === 0}
        <p class="dim">{$t('investmentCases.decisions.empty')}</p>
      {:else}
        <ol class="timeline">
          {#each decisions as d (d.id)}
            {#if d.what_happened}
              <!-- New change record -->
              <li class="change">
                <div class="tl-head">
                  <span class="when">{fmtDate(d.recorded_at)}</span>
                  {#if d.impact_direction}
                    <span class="impact {d.impact_direction}">
                      {$t(`investmentCases.record.impact.${d.impact_direction}`)}
                    </span>
                  {/if}
                </div>
                <p class="what">{d.what_happened}</p>
                {#if d.affected_assumption_text}
                  <p class="field"><b>{$t('investmentCases.record.assumption')}</b> {d.affected_assumption_text}</p>
                {/if}
                <div class="snap">
                  <span>{$t(`investmentCases.decision.type.${d.decision_type}`)}</span>
                  <span>
                    {$t('investmentCases.state.confidence')} {d.confidence_before ?? '—'} → {d.confidence_after ?? d.confidence}/5
                  </span>
                  <span>{$t('investmentCases.decisions.then')} {d.snapshot_price != null ? fmtMoney(d.snapshot_price, d.snapshot_currency) : '—'}</span>
                  <span>{d.snapshot_quantity}</span>
                  <span class:up={d.snapshot_unrealized_pnl > 0} class:down={d.snapshot_unrealized_pnl < 0}>
                    {d.snapshot_unrealized_pnl != null ? fmtSignedMoney(d.snapshot_unrealized_pnl, d.snapshot_currency) : '—'}
                  </span>
                </div>
              </li>
            {:else}
              <!-- Legacy decision record -->
              <li>
                <div class="tl-head">
                  <strong>{$t(`investmentCases.decision.type.${d.decision_type}`)}</strong>
                  <span class="conf">{$t('investmentCases.decisions.confidence')}: {d.confidence}/5</span>
                  <span class="when">{fmtDate(d.recorded_at)}</span>
                </div>
                <div class="snap">
                  <span>{$t('investmentCases.decisions.then')}</span>
                  <span>{d.snapshot_price != null ? fmtMoney(d.snapshot_price, d.snapshot_currency) : '—'}</span>
                  <span>{d.snapshot_quantity}</span>
                  <span>{$t('investmentCases.decisions.cost')}: {d.snapshot_avg_cost != null ? fmtMoney(d.snapshot_avg_cost, d.snapshot_currency) : '—'}</span>
                  <span class:up={d.snapshot_unrealized_pnl > 0} class:down={d.snapshot_unrealized_pnl < 0}>
                    {d.snapshot_unrealized_pnl != null ? fmtSignedMoney(d.snapshot_unrealized_pnl, d.snapshot_currency) : '—'}
                  </span>
                </div>
                {#if d.reason}<p class="field"><b>{$t('investmentCases.decision.reason')}</b> {d.reason}</p>{/if}
                {#if d.evidence}<p class="field"><b>{$t('investmentCases.decision.evidence')}</b> {d.evidence}</p>{/if}
                {#if d.counter_evidence}<p class="field"><b>{$t('investmentCases.decision.counterEvidence')}</b> {d.counter_evidence}</p>{/if}
                {#if d.invalidation}<p class="field"><b>{$t('investmentCases.decision.invalidation')}</b> {d.invalidation}</p>{/if}
                {#if d.watch_items}<p class="field"><b>{$t('investmentCases.decision.watchItems')}</b> {d.watch_items}</p>{/if}
              </li>
            {/if}
          {/each}
        </ol>
      {/if}
    </section>

    <!-- 6. Portfolio operations (Portfolio Tracker SSOT). -->
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

    <!-- 7. Supplementary research notes (existing Editor, collapsed). -->
    <section class="card research">
      <div
        class="sec-head clickable"
        role="button"
        tabindex="0"
        onclick={() => (notesOpen = !notesOpen)}
      >
        <h2>{$t('investmentCases.notes.title')}</h2>
        <span class="fold">
          {notesOpen ? '▾' : '▸'}
          {#if notesOpen && saveState === 'saving'}<span class="dim">{$t('investmentCases.research.saving')}</span>{/if}
          {#if notesOpen && saveState === 'saved'}<span class="dim">{$t('investmentCases.research.saved')}</span>{/if}
          {#if notesOpen && saveState === 'error'}<span class="dim">{$t('investmentCases.research.saveError')}</span>{/if}
        </span>
      </div>
      {#if notesOpen}
        <p class="hint">{$t('investmentCases.notes.hint')}</p>
        <div class="doc-title">
          <input bind:value={doc.title} oninput={onTitleChange} placeholder={$t('investmentCases.research.untitled')} />
        </div>
        <div class="editor-wrap">
          <Editor
            bind:this={editorRef}
            content={doc.content ?? null}
            onChange={onEditorChange}
            layout={doc.layout ?? 'normal'}
          />
        </div>
      {/if}
    </section>
  </div>
{/if}

<style>
  .pad { padding: var(--space-4); }
  .err { color: var(--red); font-size: var(--font-sm); }
  .case {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    padding: var(--space-4);
    overflow-y: auto;
  }
  .case > * { flex-shrink: 0; }
  .head h1 { margin: 0; font-size: var(--font-xl); }
  .name { color: var(--text-dim); font-weight: normal; margin-left: var(--space-2); }
  .meta { display: flex; gap: var(--space-3); color: var(--text-dim); font-size: var(--font-sm); margin-top: var(--space-1); }
  .card { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: var(--space-4); }
  .card h2 { margin: 0 0 var(--space-3); font-size: var(--font-md); }
  .sec-head { display: flex; align-items: center; justify-content: space-between; }
  .sec-head h2 { margin: 0; }
  .clickable { cursor: pointer; user-select: none; }
  .fold { color: var(--text-dim); display: flex; gap: var(--space-2); align-items: center; }
  .dim { color: var(--text-dim); }
  .hint { color: var(--text-dim); font-size: var(--font-sm); margin: 0 0 var(--space-2); }
  .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: var(--space-3); }
  .stat { display: flex; flex-direction: column; gap: var(--space-1); }
  .lbl { color: var(--text-dim); font-size: var(--font-sm); }
  .num { font-variant-numeric: tabular-nums; }
  .sub { color: var(--text-dim); font-size: var(--font-sm); }
  .up { color: var(--green); }
  .down { color: var(--red); }
  .cn .up { color: var(--red); }
  .cn .down { color: var(--green); }
  .state-row { display: flex; align-items: baseline; gap: var(--space-4); flex-wrap: wrap; }
  .state-item { display: flex; flex-direction: column; gap: var(--space-1); }
  .state-edit { display: flex; gap: var(--space-3); margin-top: var(--space-3); flex-wrap: wrap; }
  .state-edit label { display: flex; flex-direction: column; gap: var(--space-1); font-size: var(--font-sm); }
  .state-edit label.grow { flex: 1; min-width: 240px; }
  label span { color: var(--text-dim); font-size: var(--font-sm); }
  select, input[type='text'], textarea {
    background: var(--bg); color: var(--text); border: 1px solid var(--border);
    border-radius: var(--radius-sm); padding: var(--space-2); font: inherit; box-sizing: border-box;
  }
  select { max-width: 240px; }
  .thesis-view .field, .thesis-edit .field { font-size: var(--font-sm); }
  .assumptions { margin: 0 0 var(--space-2); padding-left: var(--space-4); }
  .thesis-edit { display: flex; flex-direction: column; gap: var(--space-2); }
  .thesis-edit label { display: flex; flex-direction: column; gap: var(--space-1); }
  .assumption-slots { display: flex; flex-direction: column; gap: var(--space-1); }
  .assumption-slots input { width: 100%; }
  .thesis-edit input, .thesis-edit textarea { width: 100%; }
  .actions { display: flex; gap: var(--space-2); align-items: center; margin-top: var(--space-2); }
  .quick-bar { display: flex; flex-direction: column; gap: var(--space-2); }
  .quick-bar .what { width: 100%; }
  .impacts { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .chip { border: 1px solid var(--border); background: var(--bg); color: var(--text); border-radius: var(--radius-sm); padding: var(--space-1) var(--space-3); cursor: pointer; font-size: var(--font-sm); }
  .chip.on { border-color: var(--accent); color: var(--accent); }
  .quick-extra { display: flex; align-items: center; gap: var(--space-3); flex-wrap: wrap; }
  .quick-extra .cur { font-size: var(--font-sm); }
  .quick-extra label { display: flex; align-items: center; gap: var(--space-1); font-size: var(--font-sm); }
  .link { background: none; border: none; color: var(--accent); cursor: pointer; font: inherit; font-size: var(--font-sm); padding: 0; }
  .tbl { width: 100%; border-collapse: collapse; font-size: var(--font-sm); }
  .tbl th, .tbl td { text-align: left; padding: var(--space-2); border-bottom: 1px solid var(--border); }
  .tbl td.num { text-align: right; font-variant-numeric: tabular-nums; }
  .timeline { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: var(--space-3); }
  .timeline li { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: var(--space-3); }
  .tl-head { display: flex; gap: var(--space-3); align-items: baseline; flex-wrap: wrap; }
  .when, .conf { color: var(--text-dim); font-size: var(--font-sm); }
  .impact { font-size: var(--font-sm); border-radius: var(--radius-sm); padding: 0 var(--space-2); border: 1px solid var(--border); }
  .impact.strengthen { color: var(--green); border-color: var(--green); }
  .cn .impact.strengthen { color: var(--red); border-color: var(--red); }
  .impact.weaken { color: var(--red); border-color: var(--red); }
  .cn .impact.weaken { color: var(--green); border-color: var(--green); }
  .impact.neutral { color: var(--text-dim); }
  .what { margin: var(--space-2) 0; font-weight: 600; white-space: pre-wrap; }
  .snap { display: flex; gap: var(--space-3); flex-wrap: wrap; background: var(--bg); border-radius: var(--radius-sm); padding: var(--space-2); margin: var(--space-2) 0; font-size: var(--font-sm); font-variant-numeric: tabular-nums; }
  .snap span:first-child { color: var(--text-dim); }
  .field { margin: var(--space-1) 0; font-size: var(--font-sm); white-space: pre-wrap; }
  .field b { color: var(--text-dim); font-weight: 500; }
  .doc-title input { width: 100%; box-sizing: border-box; border: none; background: transparent; color: var(--text); font-size: var(--font-lg); font-weight: 600; padding: var(--space-1) 0; margin-bottom: var(--space-2); }
  .editor-wrap { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: var(--space-2); min-height: 480px; }
</style>
