<script>
  // Human-readable rendering for the free-form JSON objects the agent records in a run
  // (`market_facts`, `market_expectations`, `evidence`, `risk_assessment`). The raw JSON is
  // available behind a disclosure for debugging, but it is not the default reading view.
  //
  // Agents also write bookkeeping into these objects (ids, document versions, account
  // snapshot copies, counters). Those keys are not schema fields — they are the agent's
  // payload — so they stay in the immutable run and are only folded under "Technical
  // fields" here; nothing is discarded.
  import { t } from '$lib/i18n';

  let { value } = $props();

  const isObj = (v) => v !== null && typeof v === 'object';

  function scalar(v) {
    if (v === null || v === undefined || v === '') return '—';
    if (typeof v === 'string') return v;
    return String(v);
  }

  function label(key) {
    return String(key).replaceAll('_', ' ');
  }

  /** Keys that are run/account/document bookkeeping rather than market facts. */
  function isMetaKey(key) {
    const k = String(key);
    if (k === 'id' || k.endsWith('_id')) return true;
    if (k.startsWith('document_') || k.startsWith('global_document_') || k.startsWith('otw_document_')) {
      return true;
    }
    if (k.startsWith('checked_at_')) return true;
    if (['order_count', 'orders_count', 'runs_count_before', 'historical_runs_before_this'].includes(k)) {
      return true;
    }
    if (/^(cash|equity|initial_cash|fees|fees_paid|fee_paid|cost_basis)_/.test(k)) return true;
    if (['max_drawdown_pct', 'verified_new_account_max_drawdown_pct', 'return_pct'].includes(k)) return true;
    if (k.startsWith('automation_') || ['schedule', 'analysis_type', 'check_type', 'otw_context_asof_utc'].includes(k)) {
      return true;
    }
    return false;
  }

  function rows(v, depth = 0, inheritedMeta = false) {
    if (!isObj(v)) return [{ depth, key: '', text: scalar(v), meta: inheritedMeta }];
    if (Array.isArray(v)) {
      if (!v.length) return [{ depth, key: '', text: '—', meta: inheritedMeta }];
      return v.flatMap((item) =>
        isObj(item)
          ? rows(item, depth + 1, inheritedMeta)
          : [{ depth, key: '', text: scalar(item), meta: inheritedMeta }]
      );
    }
    const keys = Object.keys(v);
    if (!keys.length) return [{ depth, key: '', text: '—', meta: inheritedMeta }];
    const out = [];
    for (const key of keys) {
      const val = v[key];
      const meta = inheritedMeta || isMetaKey(key);
      if (isObj(val)) {
        if (Array.isArray(val) && val.every((item) => !isObj(item))) {
          out.push({ depth, key, text: val.length ? val.map(scalar).join(' · ') : '—', meta });
        } else if (!Array.isArray(val) && Object.keys(val).length === 0) {
          out.push({ depth, key, text: '—', meta });
        } else {
          out.push({ depth, key, text: '', meta });
          out.push(...rows(val, depth + 1, meta));
        }
      } else {
        out.push({ depth, key, text: scalar(val), meta });
      }
    }
    return out;
  }

  const entries = $derived(rows(value, 0));
  const primary = $derived(entries.filter((row) => !row.meta));
  const technical = $derived(entries.filter((row) => row.meta));
  const hasRaw = $derived(isObj(value) && Object.keys(value).length > 0);
</script>

<div class="facts">
  {#each primary as row}
    <div class="fact" class:no-key={!row.key} style:--depth={row.depth}>
      {#if row.key}<span class="k">{label(row.key)}</span>{/if}
      <span class="v">{row.text}</span>
    </div>
  {/each}
  {#if !primary.length && !technical.length}
    <span class="v">—</span>
  {/if}
</div>

{#if technical.length}
  <details class="meta" open={!primary.length}>
    <summary>{$t('paper.technical_fields')} ({technical.length})</summary>
    <div class="facts">
      {#each technical as row}
        <div class="fact" class:no-key={!row.key} style:--depth={row.depth}>
          {#if row.key}<span class="k">{label(row.key)}</span>{/if}
          <span class="v">{row.text}</span>
        </div>
      {/each}
    </div>
  </details>
{/if}

{#if hasRaw}
  <details class="raw">
    <summary>{$t('paper.raw_json')}</summary>
    <pre>{JSON.stringify(value, null, 2)}</pre>
  </details>
{/if}

<style>
  .facts {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0 0 var(--space-3);
  }
  .fact {
    display: grid;
    grid-template-columns: minmax(120px, 28%) minmax(0, 1fr);
    gap: var(--space-2);
    padding-left: calc(var(--depth) * var(--space-4));
  }
  .fact.no-key {
    grid-template-columns: 1fr;
  }
  .k {
    color: var(--dim);
    font-size: var(--text-xs);
    line-height: 1.5;
  }
  .v {
    color: var(--text);
    font-size: var(--text-sm);
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .meta,
  .raw {
    margin-top: var(--space-2);
  }
  .meta summary,
  .raw summary {
    cursor: pointer;
    color: var(--dim);
    font-size: var(--text-xs);
  }
  .meta .facts {
    margin-top: var(--space-2);
    margin-bottom: 0;
  }
  .raw pre {
    margin: var(--space-2) 0 0;
    padding: var(--space-2);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--dim);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    overflow-x: auto;
  }
</style>
