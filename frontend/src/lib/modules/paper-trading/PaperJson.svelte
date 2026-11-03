<script>
  // Human-readable rendering for the free-form JSON objects the agent records in a run
  // (`market_facts`, `market_expectations`, `evidence`, `risk_assessment`). The raw JSON is
  // available behind a disclosure for debugging, but it is not the default reading view.
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

  function rows(v, depth = 0) {
    if (!isObj(v)) return [{ depth, key: '', text: scalar(v) }];
    if (Array.isArray(v)) {
      if (!v.length) return [{ depth, key: '', text: '—' }];
      return v.flatMap((item) => (isObj(item) ? rows(item, depth + 1) : [{ depth, key: '', text: scalar(item) }]));
    }
    const keys = Object.keys(v);
    if (!keys.length) return [{ depth, key: '', text: '—' }];
    const out = [];
    for (const key of keys) {
      const val = v[key];
      if (isObj(val)) {
        if (Array.isArray(val) && val.every((item) => !isObj(item))) {
          out.push({ depth, key, text: val.length ? val.map(scalar).join(' · ') : '—' });
        } else if (!Array.isArray(val) && Object.keys(val).length === 0) {
          out.push({ depth, key, text: '—' });
        } else {
          out.push({ depth, key, text: '' });
          out.push(...rows(val, depth + 1));
        }
      } else {
        out.push({ depth, key, text: scalar(val) });
      }
    }
    return out;
  }

  const entries = $derived(rows(value, 0));
  const hasRaw = $derived(isObj(value) && Object.keys(value).length > 0);
</script>

<div class="facts">
  {#each entries as row}
    <div class="fact" class:no-key={!row.key} style:--depth={row.depth}>
      {#if row.key}<span class="k">{label(row.key)}</span>{/if}
      <span class="v">{row.text}</span>
    </div>
  {/each}
</div>

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
  .raw {
    margin-top: var(--space-2);
  }
  .raw summary {
    cursor: pointer;
    color: var(--dim);
    font-size: var(--text-xs);
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
