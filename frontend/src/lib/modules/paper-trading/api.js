/**
 * AI Paper Trading API client.
 *
 * A strategy owns one isolated Portfolio Tracker portfolio. Setup/status changes are
 * session-only; runs and orders are normally written by the OAuth MCP client (ChatGPT),
 * but this page only reads them so the ledger stays the single source of truth.
 */
import { redirectIfUnauthorized } from '$lib/auth.js';

async function req(path, options = {}) {
  const res = await fetch(`/api${path}`, {
    headers: { 'content-type': 'application/json' },
    ...options
  });
  let body = null;
  try {
    body = await res.json();
  } catch {
    /* empty */
  }
  redirectIfUnauthorized(res);
  if (!res.ok) throw new Error(body?.error ?? `request failed (${res.status})`);
  return body;
}

export const paperTradingApi = {
  list: () => req('/paper-trading/strategies').then((r) => r.strategies ?? []),
  detail: (id) => req(`/paper-trading/strategies/${id}`),
  create: (payload) =>
    req('/paper-trading/strategies', { method: 'POST', body: JSON.stringify(payload) }).then(
      (r) => r.strategy
    ),
  setStatus: (id, status) =>
    req(`/paper-trading/strategies/${id}`, {
      method: 'PATCH',
      body: JSON.stringify({ status })
    }).then((r) => r.strategy),
  runs: (id, limit = 20) =>
    req(`/paper-trading/strategies/${id}/runs?limit=${limit}`).then((r) => r.runs ?? []),
  orders: (id, limit = 20) =>
    req(`/paper-trading/strategies/${id}/orders?limit=${limit}`).then((r) => r.orders ?? [])
};

/** Human status label key. */
export const STATUS_KEY = {
  active: 'paper.status.active',
  paused: 'paper.status.paused',
  ended: 'paper.status.ended'
};

/** Currency amount, or an em dash when the value is unknown. */
export function money(value, currency) {
  if (value == null || !Number.isFinite(Number(value))) return '—';
  try {
    return new Intl.NumberFormat(undefined, {
      style: 'currency',
      currency: currency || 'USD',
      maximumFractionDigits: 2
    }).format(Number(value));
  } catch {
    return `${Number(value).toFixed(2)} ${currency ?? ''}`.trim();
  }
}

/** Quantity with up to four decimals. */
export function qty(value) {
  if (value == null || !Number.isFinite(Number(value))) return '—';
  return new Intl.NumberFormat(undefined, { maximumFractionDigits: 4 }).format(Number(value));
}

/** Fractions (0.5) shown as percentages (50%). */
export function pct(value) {
  if (value == null || !Number.isFinite(Number(value))) return '—';
  return `${(Number(value) * 100).toFixed(0)}%`;
}

/** RFC3339 timestamp in the browser locale. */
export function when(iso) {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '—';
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short'
  }).format(d);
}
