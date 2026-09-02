/** Investment Case API client (Phase 1 MVP).
 *
 * Cases are per (user, provider, provider_id) and always reached through
 * `ensure` from a portfolio asset row — there is no query-by-instrument API.
 * Live positions are aggregated server-side from the Portfolio Tracker; the
 * research document content stays with the Editor API (`docsApi`). */
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
  if (!res.ok) {
    const err = new Error(body?.error ?? `request failed (${res.status})`);
    err.status = res.status;
    throw err;
  }
  return body;
}

export const investmentCasesApi = {
  /** Ensure a case exists for the instrument behind this portfolio asset. */
  ensure: (portfolioAssetId) =>
    req('/investment-cases', {
      method: 'POST',
      body: JSON.stringify({ portfolio_asset_id: portfolioAssetId })
    }).then((r) => r.case),
  get: (id) => req(`/investment-cases/${id}`).then((r) => r.case),
  live: (id) => req(`/investment-cases/${id}/live`),
  decisions: (id) =>
    req(`/investment-cases/${id}/decisions`).then((r) => r.decisions),
  addDecision: (id, body) =>
    req(`/investment-cases/${id}/decisions`, {
      method: 'POST',
      body: JSON.stringify(body)
    }).then((r) => r.decision),
  /** Standalone-autosaved current state (valuation view / biggest question only). */
  patchState: (id, body) =>
    req(`/investment-cases/${id}/state`, {
      method: 'PATCH',
      body: JSON.stringify(body)
    }).then((r) => r.case),
  getThesis: (id) => req(`/investment-cases/${id}/thesis`),
  putThesis: (id, body) =>
    req(`/investment-cases/${id}/thesis`, {
      method: 'PUT',
      body: JSON.stringify(body)
    }).then((r) => ({ thesis: r.thesis, assumptions: r.assumptions }))
};

export { fmtMoney, fmtSignedMoney, fmtPct } from '$lib/format.js';
