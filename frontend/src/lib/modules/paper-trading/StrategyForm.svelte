<script>
  // Create a paper strategy + its isolated Portfolio. Deliberately session-only: the MCP
  // agent may trade inside an existing strategy, never create new accounts.
  import Modal from '$lib/ui/Modal.svelte';
  import Button from '$lib/ui/Button.svelte';
  import ErrorText from '$lib/ui/ErrorText.svelte';
  import { t } from '$lib/i18n';

  let { open = $bindable(false), saving = false, onsubmit = async () => {} } = $props();

  const today = new Date().toISOString().slice(0, 10);
  let name = $state('ChatGPT Paper Test');
  let currency = $state('CNY');
  let initialCash = $state(100000);
  let startDate = $state(today);
  let maxPositionPct = $state(0.5);
  let maxOrderValue = $state('');
  let slippageBps = $state(5);
  let feeRateBps = $state(3);
  let sellExtraFeeRateBps = $state(5);
  let minFee = $state(5);
  let error = $state('');

  async function submit() {
    error = '';
    if (!name.trim()) {
      error = $t('paper.form.name_required');
      return;
    }
    if (!(Number(initialCash) > 0)) {
      error = $t('paper.form.cash_required');
      return;
    }
    const weight = Number(maxPositionPct);
    if (!(weight > 0 && weight <= 1)) {
      error = $t('paper.form.weight_invalid');
      return;
    }
    const payload = {
      name: name.trim(),
      currency,
      initial_cash: Number(initialCash),
      max_position_pct: weight,
      slippage_bps: Number(slippageBps),
      fee_rate_bps: Number(feeRateBps),
      sell_extra_fee_rate_bps: Number(sellExtraFeeRateBps),
      min_fee: Number(minFee)
    };
    if (startDate) payload.start_date = startDate;
    if (String(maxOrderValue).trim() !== '') payload.max_order_value = Number(maxOrderValue);
    try {
      await onsubmit(payload);
    } catch (e) {
      error = e.message;
    }
  }
</script>

<Modal bind:open title={$t('paper.form.title')} size="md">
  <div class="form">
    <label>
      <span>{$t('paper.form.name')}</span>
      <input bind:value={name} maxlength="100" autocomplete="off" />
    </label>

    <div class="row">
      <label>
        <span>{$t('paper.form.currency')}</span>
        <select bind:value={currency}>
          <option value="CNY">CNY</option>
          <option value="USD">USD</option>
        </select>
      </label>
      <label>
        <span>{$t('paper.form.initial_cash')}</span>
        <input type="number" min="0" step="1000" bind:value={initialCash} />
      </label>
    </div>

    <div class="row">
      <label>
        <span>{$t('paper.form.start_date')}</span>
        <input type="date" bind:value={startDate} />
      </label>
      <label>
        <span>{$t('paper.form.max_position_pct')}</span>
        <input type="number" min="0.01" max="1" step="0.05" bind:value={maxPositionPct} />
      </label>
    </div>

    <label>
      <span>{$t('paper.form.max_order_value')}</span>
      <input type="number" min="0" step="1000" bind:value={maxOrderValue} placeholder="30000" />
      <small>{$t('paper.form.max_order_value_hint')}</small>
    </label>

    <details>
      <summary>{$t('paper.form.advanced')}</summary>
      <div class="row">
        <label>
          <span>{$t('paper.form.slippage_bps')}</span>
          <input type="number" min="0" max="1000" step="1" bind:value={slippageBps} />
        </label>
        <label>
          <span>{$t('paper.form.fee_rate_bps')}</span>
          <input type="number" min="0" max="1000" step="1" bind:value={feeRateBps} />
        </label>
      </div>
      <div class="row">
        <label>
          <span>{$t('paper.form.sell_extra_fee_rate_bps')}</span>
          <input type="number" min="0" max="1000" step="1" bind:value={sellExtraFeeRateBps} />
        </label>
        <label>
          <span>{$t('paper.form.min_fee')}</span>
          <input type="number" min="0" step="1" bind:value={minFee} />
        </label>
      </div>
    </details>

    <ErrorText {error} />
  </div>

  {#snippet footer()}
    <Button variant="ghost" onclick={() => (open = false)}>{$t('common.cancel')}</Button>
    <Button variant="primary" loading={saving} onclick={submit}>{$t('paper.form.create')}</Button>
  {/snippet}
</Modal>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-4);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    min-width: 0;
  }
  label > span {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  input,
  select {
    width: 100%;
    box-sizing: border-box;
  }
  small {
    color: var(--dim);
    font-size: var(--text-xs);
  }
  details {
    border-top: 1px solid var(--border);
    padding-top: var(--space-3);
  }
  summary {
    cursor: pointer;
    color: var(--dim);
    font-size: var(--text-sm);
    margin-bottom: var(--space-3);
  }
  @media (max-width: 560px) {
    .row {
      grid-template-columns: 1fr;
    }
  }
</style>
