<script>
  // Decision record modal: user fields only. Snapshot numbers are computed
  // server-side and never accepted from the client.
  import Modal from '$lib/ui/Modal.svelte';
  import { t } from '$lib/i18n';

  let {
    open = false,
    onclose = null,
    onsubmit = null
  } = $props();

  let decision_type = $state('hold');
  let confidence = $state(3);
  let reason = $state('');
  let evidence = $state('');
  let counter_evidence = $state('');
  let invalidation = $state('');
  let watch_items = $state('');
  let busy = $state(false);
  let err = $state(null);

  const TYPES = ['observe', 'hold', 'buy', 'add', 'reduce', 'exit'];

  async function submit() {
    busy = true;
    err = null;
    try {
      await onsubmit?.({
        decision_type,
        confidence,
        reason: reason.trim(),
        evidence: evidence.trim(),
        counter_evidence: counter_evidence.trim(),
        invalidation: invalidation.trim(),
        watch_items: watch_items.trim()
      });
    } catch (e) {
      err = e.message;
    } finally {
      busy = false;
    }
  }
</script>

<Modal open={open} size="md" title={$t('investmentCases.decision.title')} onclose={onclose}>
  <div class="form">
    <div class="row">
      <label>
        <span>{$t('investmentCases.decision.type')}</span>
        <select bind:value={decision_type}>
          {#each TYPES as ty}
            <option value={ty}>{$t(`investmentCases.decision.type.${ty}`)}</option>
          {/each}
        </select>
      </label>
      <label>
        <span>{$t('investmentCases.decision.confidence')}</span>
        <select bind:value={confidence}>
          {#each [1, 2, 3, 4, 5] as c}
            <option value={c}>{c} / 5</option>
          {/each}
        </select>
      </label>
    </div>
    <label>
      <span>{$t('investmentCases.decision.reason')}</span>
      <textarea bind:value={reason} rows="3"></textarea>
    </label>
    <label>
      <span>{$t('investmentCases.decision.evidence')}</span>
      <textarea bind:value={evidence} rows="2"></textarea>
    </label>
    <label>
      <span>{$t('investmentCases.decision.counterEvidence')}</span>
      <textarea bind:value={counter_evidence} rows="2"></textarea>
    </label>
    <label>
      <span>{$t('investmentCases.decision.invalidation')}</span>
      <textarea bind:value={invalidation} rows="2"></textarea>
    </label>
    <label>
      <span>{$t('investmentCases.decision.watchItems')}</span>
      <textarea bind:value={watch_items} rows="2"></textarea>
    </label>
    {#if err}<p class="err">{err}</p>{/if}
  </div>
  <div class="actions">
    <button class="btn" disabled={busy} onclick={submit}>
      {busy ? '…' : $t('investmentCases.decision.save')}
    </button>
    <button class="btn ghost" onclick={onclose}>{$t('common.cancel')}</button>
  </div>
</Modal>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }
  .row {
    display: flex;
    gap: var(--space-3);
  }
  .row label {
    flex: 1;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    font-size: var(--font-sm);
  }
  label span {
    color: var(--text-dim);
  }
  select,
  textarea {
    width: 100%;
    box-sizing: border-box;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: var(--space-2);
    font: inherit;
  }
  .err {
    color: var(--red);
    font-size: var(--font-sm);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-3);
  }
</style>
