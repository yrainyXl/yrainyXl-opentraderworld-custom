/**
 * Price-observation data helpers: turning a watchlist item's cached Market bars into a
 * dated 30-day series, and aligning user observations onto that series by *market date*.
 *
 * The single source of the daily line is the item's own quote payload — the same
 * `history` ([[unix_sec, close], …]) the server already computed the row's sparkline
 * from. Nothing is fetched or copied for the modal; observations only ride on top.
 *
 * `recorded_at` carries a time of day while the series is daily bars, so an observation
 * is aligned by its date *in the market's timezone* (Asia/Shanghai for A-shares,
 * America/New_York for US Yahoo equities/ETFs, UTC for crypto, Asia/Hong_Kong for .HK).
 * An observation on a non-trading date (weekend/holiday) lands on the nearest earlier
 * bar so its marker stays visible on the line.
 */

// ── Market timezone ───────────────────────────────────────────────────────────

/** Best-guess market timezone for an item's daily bars. Falls back to UTC. */
export function marketTimeZone(item) {
  if (!item) return 'UTC';
  if (item.provider === 'china') return 'Asia/Shanghai';
  if (item.asset_class === 'crypto') return 'UTC';
  const ex = String(item.exchange ?? '').toUpperCase();
  if (ex.includes('SH') || ex.includes('SSE') || ex.includes('SHZ') || ex.includes('BJ')) {
    return 'Asia/Shanghai';
  }
  if (ex.includes('HK')) return 'Asia/Hong_Kong';
  // Yahoo equities/ETFs default to US sessions.
  return 'America/New_York';
}

const fmtCache = new Map();
function tzFmt(tz, opts) {
  const key = `${tz}|${JSON.stringify(opts)}`;
  let f = fmtCache.get(key);
  if (!f) {
    try {
      f = new Intl.DateTimeFormat('en-US', { timeZone: tz, ...opts });
    } catch {
      f = null; // unknown tz → fall back to the browser's local zone below
    }
    fmtCache.set(key, f);
  }
  return f;
}

function toMs(v) {
  if (typeof v === 'number') return v < 1e12 ? v * 1000 : v; // unix_sec vs unix_ms
  return Date.parse(v); // RFC3339 instants
}

/**
 * "YYYY-MM-DD" for an instant, in the given market timezone (local zone on failure).
 *
 * The parts are read out individually instead of formatting with a locale pattern: an
 * `en-US` pattern renders MM/DD/YYYY, so `slice(5)` (used for the "09-08" day labels and
 * for chronological key comparison) would have yielded the year. Parts always give the
 * ISO field order regardless of the engine's locale data.
 */
export function tzDateKey(v, tz) {
  const f = tzFmt(tz, { year: 'numeric', month: '2-digit', day: '2-digit' });
  if (f) {
    const parts = f.formatToParts(new Date(toMs(v)));
    const get = (type) => parts.find((p) => p.type === type)?.value ?? '';
    const y = get('year');
    const m = get('month');
    const d = get('day');
    if (y && m && d) return `${y}-${m.padStart(2, '0')}-${d.padStart(2, '0')}`;
  }
  const fallback = new Date(toMs(v));
  const mm = String(fallback.getMonth() + 1).padStart(2, '0');
  const dd = String(fallback.getDate()).padStart(2, '0');
  return `${fallback.getFullYear()}-${mm}-${dd}`;
}

/** "HH:MM" (24h) for an instant, in the given market timezone. */
export function tzTimeLabel(v, tz) {
  const f = tzFmt(tz, {
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23'
  });
  if (f) return f.format(new Date(toMs(v)));
  return new Date(toMs(v)).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
}

/** "M/D" short axis label for an instant, in the given market timezone. */
export function tzDayLabel(v, tz) {
  const f = tzFmt(tz, { month: 'numeric', day: 'numeric' });
  if (f) return f.format(new Date(toMs(v)));
  return `${new Date(toMs(v)).getMonth() + 1}/${new Date(toMs(v)).getDate()}`;
}

/** Full "M/D/YYYY" title label for an instant, in the given market timezone. */
export function tzFullDate(v, tz) {
  const f = tzFmt(tz, { year: 'numeric', month: 'numeric', day: 'numeric' });
  if (f) return f.format(new Date(toMs(v)));
  return new Date(toMs(v)).toLocaleDateString();
}

// ── Daily series (mirror of the server's compute_quote over quote.history) ────

/**
 * Which *session day* an instant belongs to, for grouping daily bars and for deciding
 * what counts as "today's still-running session".
 *
 * A China A-share session (09:30–15:00 CST) sits inside one UTC date, but a CN user's day
 * starts at 00:00 CST = 16:00 UTC the previous day. Grouping by UTC days would therefore
 * treat the just-finished session's bar as "today's partial" for the whole 00:00–08:00 CST
 * window and hide it — the previous session's price would vanish from the popup (and its
 * date label would be borrowed by the live point). China rows therefore group on the
 * market's own date (UTC+8, no DST); other providers keep the previous UTC-day behaviour.
 */
function sessionDay(secs, provider) {
  return provider === 'china' ? Math.floor((secs + 8 * 3600) / 86_400) : Math.floor(secs / 86_400);
}

/**
 * The modal's 30-day line, derived from the cached Market bars exactly like the row's
 * sparkline: collapse the raw provider series to one close per session day (last value of
 * the day), drop the still-running session, keep the newest ~30 completed closes, then
 * append the live spot as the final point — but only when it carries something new. After
 * the close the "live" value is just the last close again, so appending it would add a
 * phantom today row labelled with the current date and holding yesterday's price.
 * Returns [{ ts, close, live }] or [] when the item has no usable bars.
 */
export function dailySeriesFromItem(item, nowMs = Date.now()) {
  const quote = item?.quote;
  const raw = quote?.history;
  const live = quote?.price_usd;
  const provider = item?.provider;
  if (!Array.isArray(raw) || raw.length === 0 || !isFinite(live)) return [];
  const sorted = [...raw].sort((a, b) => a[0] - b[0]);
  const daily = [];
  for (const [t, c] of sorted) {
    if (!isFinite(c)) continue;
    const day = sessionDay(t, provider);
    const last = daily[daily.length - 1];
    if (last && sessionDay(last.ts, provider) === day) {
      last.ts = t;
      last.close = c;
    } else {
      daily.push({ ts: t, close: c });
    }
  }
  if (daily.length === 0) return [];
  const nowSec = Math.floor(nowMs / 1000);
  const todayDay = sessionDay(nowSec, provider);
  const completed = daily.filter((p) => sessionDay(p.ts, provider) < todayDay);
  const pts = completed.slice(-30).map((p) => ({ ts: p.ts, close: p.close, live: false }));
  const lastClose = pts.length ? pts[pts.length - 1].close : null;
  const liveIsRepeat =
    lastClose != null && Math.abs(live - lastClose) <= Math.abs(lastClose) * 1e-9;
  if (!liveIsRepeat) {
    pts.push({ ts: nowSec, close: live, live: true });
  }
  return pts;
}

// ── Observation → daily-point alignment ──────────────────────────────────────

/**
 * Attach observations to daily points by market date: a point whose date matches the
 * observation's date holds it; otherwise the observation lands on the nearest earlier
 * bar (so weekend/holiday records still show), falling back to the first bar. Returns a
 * Map<pointIndex, observation[]>, newest observation first within each day.
 */
export function groupObservationsByPoint(pts, observations, tz) {
  const groups = new Map();
  if (!observations?.length || pts.length === 0) return groups;
  const dates = pts.map((p) => tzDateKey(p.ts, tz));
  const byDate = new Map(); // dateKey -> observations (newest first)
  for (const o of observations) {
    const key = tzDateKey(o.recorded_at, tz);
    const list = byDate.get(key) ?? [];
    list.push(o);
    byDate.set(key, list);
  }
  for (const [key, list] of byDate) {
    list.sort((a, b) => Date.parse(b.recorded_at) - Date.parse(a.recorded_at));
    let idx = dates.indexOf(key);
    if (idx === -1) {
      // No bar on that date (weekend/holiday or pre-market): nearest earlier bar.
      for (let i = dates.length - 1; i >= 0; i--) {
        if (dates[i] < key) {
          idx = i;
          break;
        }
      }
      if (idx === -1) idx = 0;
    }
    const group = groups.get(idx) ?? [];
    group.push(...list);
    groups.set(idx, group);
  }
  return groups;
}
