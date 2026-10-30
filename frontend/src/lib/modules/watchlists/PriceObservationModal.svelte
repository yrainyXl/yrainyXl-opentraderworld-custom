<script>
  // Price Observation modal — opened by clicking a watchlist row's 30-day sparkline.
  //
  // One window over a symbol that answers "what happened / what did I note" without leaving
  // the Watchlist:
  //   1. the system's 30-day line (reused Market bars from the item's cached quote),
  //   2. the user's own observations as markers on that line,
  //   3. a quick "+ 记录当前价格" inline editor (note only; price/time are server-frozen),
  //   4. edit/delete on each observation (note edits only — the snapshot stays stable),
  //   5. a collapsible daily-price table fed by the same bars.
  //
  // Everything is lazy: opening the modal is the only moment observations are fetched, the
  // bars are whatever the Watchlist row already carries (no duplicate request), and a
  // failure in either area never affects the Watchlist page underneath.
  import Modal from '$lib/ui/Modal.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Icon from '$lib/ui/Icon.svelte';
  import ErrorText from '$lib/ui/ErrorText.svelte';
  import ConfirmModal from '$lib/ui/ConfirmModal.svelte';
  import { watchlistsApi, fmtQuote, fmtSignedPct, agoLabel, isChinaItem } from './api.js';
  import {
    marketTimeZone,
    tzDateKey,
    tzTimeLabel,
    dailySeriesFromItem,
    groupObservationsByPoint
  } from './observations.js';
  import PriceTrendChart from './PriceTrendChart.svelte';
  import { t, locale } from '$lib/i18n';
  import { fmtTime } from '$lib/format.js';

  let { open = $bindable(false), item = null, onchanged = () => {} } = $props();

  // ── Derived display context (from the item the Watchlist row already holds) ──
  const tz = $derived(marketTimeZone(item));
  const china = $derived(isChinaItem(item));
  const currency = $derived(
    typeof item?.quote?.currency === 'string' && item.quote.currency
      ? item.quote.currency
      : item?.provider === 'china'
        ? 'CNY'
        : 'USD'
  );
  const price = $derived(item?.quote?.price_usd);
  const change24 = $derived(item?.quote?.change_24h);
  const quotedAt = $derived(item?.quoted_at);
  const hasQuoteError = $derived(!!item?.quote?.error);

  const series = $derived(item ? dailySeriesFromItem(item) : []);
  const groups = $derived(item ? groupObservationsByPoint(series, observations ?? [], tz) : new Map());
  const barsAvailable = $derived(series.length >= 2);

  // ── Observations state (lazy, per open) ───────────────────────────────────
  let observations = $state(null); // null = not loaded yet
  let obsLoading = $state(false);
  let obsError = $state('');
  let loadTick = $state(0);

  // Quick-record inline editor.
  let recordOpen = $state(false);
  let noteDraft = $state('');
  let saving = $state(false);
  let recordError = $state('');

  // Inline note editing of an existing observation.
  let editingId = $state(null);
  let editDraft = $state('');
  let savingEdit = $state(false);
  let editError = $state('');

  // Delete confirmation.
  let confirmDeleteOpen = $state(false);
  let deleting = $state(null); // observation awaiting confirm
  let deleteError = $state('');

  // Collapsible daily price table.
  let dailyOpen = $state(false);

  $effect(() => {
    open;
    item?.id;
    // Opening for a symbol resets whatever was half-typed for the previous one.
    recordOpen = false;
    noteDraft = '';
    recordError = '';
    editingId = null;
    editDraft = '';
    editError = '';
    deleting = null;
    deleteError = '';
    if (open && item?.id) load();
  });

  async function load() {
    const itemId = item?.id;
    if (!itemId) return;
    obsLoading = true;
    obsError = '';
    try {
      observations = await watchlistsApi.listObservations(itemId);
    } catch (e) {
      obsError = e.message;
      observations = observations ?? [];
    } finally {
      obsLoading = false;
    }
  }

  function openRecord() {
    recordOpen = true;
    noteDraft = '';
    recordError = '';
  }

  async function saveRecord() {
    if (saving || !item) return;
    saving = true;
    recordError = '';
    try {
      const o = await watchlistsApi.addObservation(item.id, noteDraft.trim());
      recordOpen = false;
      // The server orders by recorded_at DESC — the new record is newest.
      observations = [o, ...(observations ?? []).filter((x) => x.id !== o.id)];
      onchanged();
    } catch (e) {
      recordError = e.message; // editor stays open with the note — retry is one click
    } finally {
      saving = false;
    }
  }

  function startEdit(o) {
    editingId = o.id;
    editDraft = o.note ?? '';
    editError = '';
  }

  function cancelEdit() {
    editingId = null;
    editError = '';
  }

  async function saveEdit(o) {
    if (savingEdit) return;
    savingEdit = true;
    editError = '';
    try {
      const updated = await watchlistsApi.updateObservation(o.id, editDraft.trim());
      observations = (observations ?? []).map((x) => (x.id === updated.id ? updated : x));
      editingId = null;
      onchanged();
    } catch (e) {
      editError = e.message;
    } finally {
      savingEdit = false;
    }
  }

  async function confirmDelete() {
    const o = deleting;
    deleting = null;
    if (!o) return;
    deleteError = '';
    try {
      await watchlistsApi.removeObservation(o.id);
      observations = (observations ?? []).filter((x) => x.id !== o.id);
      if (editingId === o.id) editingId = null;
      onchanged();
    } catch (e) {
      deleteError = e.message;
    }
  }

  // ── Timeline presentation helpers ─────────────────────────────────────────
  /** "09-08" (or "2025-09-08" when the year differs from this market's current year). */
  function dayLabel(o) {
    const key = tzDateKey(o.recorded_at, tz);
    const thisYear = tzDateKey(Date.now() / 1000, tz).slice(0, 4);
    return key.slice(0, 4) === thisYear ? key.slice(5) : key;
  }

  function rowPrice(o) {
    return fmtQuote(o.price_snapshot, o.currency || currency);
  }

  const mineAt = (i) => (groups?.get(i) ?? []).length;

  const updatedAgo = $derived.by(() => {
    void $locale; // re-derive on locale switches
    return quotedAt ? agoLabel(quotedAt, $locale) : null;
  });

  /** Now, formatted for the quick-record preview (the server fixes the real instant). */
  const nowLabel = $derived(fmtTime(new Date()));
</script>

<Modal
  bind:open
  size="lg"
  title={item ? `${item.name}  ${item.symbol}` : ''}
  onclose={() => {
    dailyOpen = false;
  }}
>
  {#if item}
    <div class="wrap">
      <!-- ── Current price strip ── -->
      <div class="strip">
        <div class="bigprice">
          {#if hasQuoteError}
            <Icon name="alert-triangle" size={13} />
          {/if}
          {fmtQuote(price, currency)}
          {#if change24 != null && isFinite(change24)}
            <span
              class="pct"
              class:up={change24 > 0}
              class:down={change24 < 0}
              class:china={china}
            >
              {fmtSignedPct(change24)}
            </span>
          {/if}
        </div>
        <span class="meta">
          {$t('watchlists.obs.updatedAgo', { ago: updatedAgo ?? '—' })}
          {#if item.exchange}· {item.exchange}{/if}
        </span>
      </div>

      <!-- ── 30-day trend (reused Market bars; user observations as markers) ── -->
      <section>
        <h3>{$t('watchlists.obs.trendTitle')}</h3>
        <PriceTrendChart
          {item}
          {currency}
          pts={series}
          {groups}
          ariaLabel={$t('watchlists.obs.chartAria', { symbol: item.symbol })}
        >
          <span slot="empty">{$t('watchlists.obs.trendUnavailable')}</span>
        </PriceTrendChart>
      </section>

      <!-- ── My observations timeline ── -->
      <section>
        <h3>
          {$t('watchlists.obs.myObservations')}
          {#if observations?.length}<span class="count">{observations.length}</span>{/if}
        </h3>

        <ErrorText error={obsError} />

        {#if obsLoading && !observations?.length}
          <p class="hint">{$t('watchlists.obs.loading')}</p>
        {:else if !observations?.length}
          <p class="hint empty">{$t('watchlists.obs.emptyList')}</p>
        {:else}
          <div class="oblist">
            {#each observations as o (o.id)}
              <div class="obrow">
                {#if editingId === o.id}
                  <!-- inline note edit — price_snapshot / recorded_at stay frozen -->
                  <div class="editbox">
                    <div class="frozen">
                      <span>{dayLabel(o)} {tzTimeLabel(o.recorded_at, tz)}</span>
                      <span class="fprice">{rowPrice(o)}</span>
                    </div>
                    <textarea
                      bind:value={editDraft}
                      rows="2"
                      maxlength="1000"
                      placeholder={$t('watchlists.obs.notePlaceholder')}
                    ></textarea>
                    <ErrorText error={editError} />
                    <div class="rowacts">
                      <span class="spacer"></span>
                      <Button size="sm" onclick={cancelEdit}>{$t('common.cancel')}</Button>
                      <Button
                        variant="primary"
                        size="sm"
                        icon="check"
                        loading={savingEdit}
                        onclick={() => saveEdit(o)}
                      >
                        {$t('common.save')}
                      </Button>
                    </div>
                  </div>
                {:else}
                  <div class="obhead">
                    <span class="when" title={tzDateKey(o.recorded_at, tz)}>
                      {dayLabel(o)} {tzTimeLabel(o.recorded_at, tz)}
                    </span>
                    <span class="oprice">{rowPrice(o)}</span>
                    <span class="spacer"></span>
                    <button class="iconbtn" onclick={() => startEdit(o)} aria-label={$t('common.edit')} title={$t('common.edit')}>
                      <Icon name="pencil" size={12} />
                    </button>
                    <button
                      class="iconbtn danger"
                      onclick={() => ((deleting = o), (confirmDeleteOpen = true))}
                      aria-label={$t('common.delete')}
                      title={$t('common.delete')}
                    >
                      <Icon name="trash" size={12} />
                    </button>
                  </div>
                  {#if (o.note ?? '').trim()}
                    <p class="onote">{(o.note ?? '').trim()}</p>
                  {/if}
                {/if}
              </div>
            {/each}
          </div>
        {/if}
        <ErrorText error={deleteError} />
      </section>

      <!-- ── Quick record (inline; no second modal) ── -->
      <section class="rec">
        {#if recordOpen}
          <div class="editor">
            <h4>{$t('watchlists.obs.addTitle')}</h4>
            <div class="fields">
              <div class="field">
                <span class="flabel">{$t('watchlists.obs.currentPrice')}</span>
                <span class="fvalue">
                  {fmtQuote(price, currency)}
                  <em>{$t('watchlists.obs.priceAuto')}</em>
                </span>
              </div>
              <div class="field">
                <span class="flabel">{$t('watchlists.obs.timeField')}</span>
                <span class="fvalue">
                  {nowLabel}
                  <em>{$t('watchlists.obs.timeAuto')}</em>
                </span>
              </div>
              <div class="field">
                <span class="flabel">{$t('watchlists.obs.noteLabel')}</span>
                <textarea
                  bind:value={noteDraft}
                  rows="2"
                  maxlength="1000"
                  placeholder={$t('watchlists.obs.notePlaceholder')}
                ></textarea>
              </div>
            </div>
            <ErrorText error={recordError} />
            <div class="rowacts">
              <span class="spacer"></span>
              <Button size="sm" onclick={() => (recordOpen = false)}>{$t('common.cancel')}</Button>
              <Button variant="primary" size="sm" icon="check" loading={saving} onclick={saveRecord}>
                {$t('watchlists.obs.recordBtn')}
              </Button>
            </div>
          </div>
        {:else}
          <Button variant="primary" icon="plus" onclick={openRecord}>
            {$t('watchlists.obs.addBtn')}
          </Button>
        {/if}
      </section>

      <!-- ── Daily system prices (same bars as the chart; default collapsed) ── -->
      <section class="daily">
        <button class="fold" class:open={dailyOpen} onclick={() => (dailyOpen = !dailyOpen)}>
          <Icon name={dailyOpen ? 'chevron-down' : 'chevron-right'} size={12} />
          {$t('watchlists.obs.dailyToggle')}
          {#if !barsAvailable}
            <span class="dim">{$t('watchlists.obs.trendUnavailable')}</span>
          {/if}
        </button>
        {#if dailyOpen}
          {#if !barsAvailable}
            <p class="hint empty">{$t('watchlists.obs.trendUnavailable')}</p>
          {:else}
            <div class="dailytable">
              <table>
                <thead>
                  <tr>
                    <th>{$t('watchlists.obs.dailyColDate')}</th>
                    <th class="num">{$t('watchlists.obs.dailyColPrice')}</th>
                    <th class="num">{ $t('watchlists.obs.dailyColMine')}</th>
                  </tr>
                </thead>
                <tbody>
                  {#each [...series].reverse() as p, ri (series.length - 1 - ri)}
                    {@const n = mineAt(series.length - 1 - ri)}
                    <tr>
                      <td class="d">{tzDateKey(p.ts, tz).slice(5)}{#if p.live}<span class="live">{$t('watchlists.obs.liveTag')}</span>{/if}</td>
                      <td class="num">{fmtQuote(p.close, currency)}</td>
                      <td class="num mine">
                        {#if n > 0}
                          <span class="dot" title={$t('watchlists.obs.mineMark', { n })}></span>
                        {/if}
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        {/if}
      </section>
    </div>
  {/if}

  <ConfirmModal
    bind:open={confirmDeleteOpen}
    title={$t('watchlists.obs.deleteTitle')}
    message={$t('watchlists.obs.deleteConfirm')}
    confirmLabel={$t('common.delete')}
    cancelLabel={$t('common.cancel')}
    danger
    onconfirm={confirmDelete}
    oncancel={() => (deleting = null)}
  />
</Modal>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  section {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  h3 {
    font-size: var(--text-xs);
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }
  .count {
    font-family: var(--mono);
    font-size: var(--text-xs);
    color: var(--muted);
    background: var(--surface-2);
    border: var(--hairline) solid var(--border);
    padding: 0 var(--space-1);
    line-height: 16px;
  }

  /* Price strip */
  .strip {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .bigprice {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    font-size: var(--text-2xl, 1.6rem);
    font-weight: var(--fw-medium);
    font-family: var(--mono);
    color: var(--text);
  }
  .pct {
    font-size: var(--text-sm);
    font-weight: var(--fw-medium);
  }
  .pct.up {
    color: var(--green);
  }
  .pct.down {
    color: var(--red);
  }
  .pct.china.up {
    color: var(--red);
  }
  .pct.china.down {
    color: var(--green);
  }
  .meta {
    font-size: var(--text-xs);
    color: var(--muted);
  }

  /* Timeline */
  .hint {
    font-size: var(--text-sm);
    color: var(--muted);
    margin: 0;
  }
  .hint.empty {
    padding: var(--space-3);
    border: var(--hairline) dashed var(--border);
  }
  .oblist {
    display: flex;
    flex-direction: column;
    max-height: 236px;
    overflow-y: auto;
    border-top: var(--hairline) solid var(--border);
  }
  .obrow {
    padding: var(--space-2) 0;
    border-bottom: var(--hairline) solid var(--border);
  }
  .obhead {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .when {
    font-family: var(--mono);
    font-size: var(--text-xs);
    color: var(--muted);
    white-space: nowrap;
  }
  .oprice {
    font-family: var(--mono);
    font-size: var(--text-sm);
    font-weight: var(--fw-medium);
    color: var(--text);
  }
  .onote {
    margin: var(--space-1) 0 0;
    font-size: var(--text-sm);
    line-height: 1.5;
    color: var(--text);
    overflow-wrap: anywhere;
    white-space: pre-line;
  }
  .spacer {
    flex: 1;
  }
  .iconbtn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    background: none;
    border: none;
    color: var(--muted);
    cursor: pointer;
  }
  .iconbtn:hover {
    color: var(--text);
  }
  .iconbtn.danger:hover {
    color: var(--red);
  }

  /* Inline editors (quick record + note edit) — never a nested modal */
  .editor,
  .editbox {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    border: var(--hairline) solid var(--border);
    background: var(--surface-2);
  }
  h4 {
    margin: 0;
    font-size: var(--text-sm);
    font-weight: var(--fw-medium);
  }
  .fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .flabel {
    font-size: var(--text-xs);
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .fvalue {
    display: inline-flex;
    align-items: baseline;
    gap: var(--space-2);
    font-family: var(--mono);
    font-size: var(--text-sm);
    color: var(--text);
  }
  .fvalue em {
    font-style: normal;
    font-size: var(--text-xs);
    color: var(--dim);
  }
  .frozen {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    font-size: var(--text-xs);
    color: var(--muted);
    font-family: var(--mono);
  }
  .fprice {
    font-weight: var(--fw-medium);
    color: var(--text);
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    resize: vertical;
    min-height: 46px;
    background: var(--surface);
    border: var(--hairline) solid var(--border-control);
    border-radius: 0;
    color: var(--text);
    font-size: var(--text-sm);
    line-height: 1.5;
    padding: var(--space-2);
    font-family: inherit;
  }
  .rowacts {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .rec {
    border-top: var(--hairline) solid var(--border);
    padding-top: var(--space-3);
  }

  /* Daily prices fold */
  .daily {
    border-top: var(--hairline) solid var(--border);
    padding-top: var(--space-2);
  }
  .fold {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    background: none;
    border: none;
    padding: 0;
    font: inherit;
    font-size: var(--text-sm);
    color: var(--text);
    cursor: pointer;
  }
  .fold:hover {
    color: var(--accent);
  }
  .dim {
    color: var(--muted);
    font-size: var(--text-xs);
  }
  .dailytable {
    max-height: 300px;
    overflow-y: auto;
    border: var(--hairline) solid var(--border);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
  }
  th {
    text-align: left;
    font-size: var(--text-xs);
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-weight: var(--fw-medium);
    padding: var(--space-1) var(--space-2);
    border-bottom: var(--hairline) solid var(--border);
    background: var(--surface-2);
    position: sticky;
    top: 0;
  }
  th.num,
  td.num {
    text-align: right;
    font-family: var(--mono);
  }
  td {
    padding: 3px var(--space-2);
    border-bottom: var(--hairline) solid var(--border);
  }
  td.d {
    color: var(--muted);
  }
  .live {
    margin-left: 6px;
    font-size: 9px;
    color: var(--accent);
    border: 1px solid var(--accent);
    padding: 0 3px;
    line-height: 13px;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  td.mine {
    text-align: center;
  }
  .dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--amber);
  }
</style>
