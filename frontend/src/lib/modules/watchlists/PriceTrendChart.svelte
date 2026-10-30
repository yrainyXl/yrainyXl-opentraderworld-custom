<script>
  // 30-day trend chart inside the Price Observation modal.
  //
  // Plain SVG, no chart library (same approach as the row's sparkline): the system's daily
  // closes form a simple polyline, and the user's own observations are drawn as markers on
  // top of their daily points (an amber dot; a numbered dot when a day holds several).
  // Hovering shows a tooltip — the daily close for an ordinary point, or the recorded
  // time/price/note for an observation marker. A-shares follow 涨红跌绿 (`china` flips the
  // palette), matching every other Watchlist chart.
  //
  // Data contract: `pts` is the dated daily series (see observations.js), `groups` maps a
  // point index to that day's observations (newest first).
  import { fmtQuote, isChinaItem } from './api.js';
  import { marketTimeZone, tzDateKey, tzTimeLabel, tzDayLabel, tzFullDate } from './observations.js';
  import { t } from '$lib/i18n';

  let { pts = [], groups = null, item = null, currency = 'USD', ariaLabel = '' } = $props();

  const tz = $derived(marketTimeZone(item));
  const china = $derived(isChinaItem(item));

  const H = 190;
  const PAD_X = 8;
  const PAD_TOP = 26; // headroom so an aggregate marker on the max point never clips
  const PAD_BOT = 22;

  // Measure the actual render width so coordinates are plain CSS px (no scaling distortion).
  let w = $state(0);

  let range = $derived.by(() => {
    if (pts.length < 2) return null;
    let lo = Infinity;
    let hi = -Infinity;
    for (const p of pts) {
      if (p.close < lo) lo = p.close;
      if (p.close > hi) hi = p.close;
    }
    const span = hi - lo || Math.max(hi * 0.01, 0.01);
    const pad = span * 0.1;
    return { min: lo - pad, max: hi + pad, span: (hi - lo) + 2 * pad };
  });

  let scale = $derived.by(() => {
    if (!range || pts.length < 2) return null;
    const xAt = (i) =>
      pts.length <= 1 ? PAD_X : PAD_X + (i * (w - 2 * PAD_X)) / (pts.length - 1);
    const yAt = (v) => PAD_TOP + ((range.max - v) / range.span) * (H - PAD_TOP - PAD_BOT);
    return { xAt, yAt };
  });

  // Color by period direction: same convention as the row sparkline (China flips the
  // palette to 涨红跌绿).
  const up = $derived(pts.length >= 2 ? pts[pts.length - 1].close >= pts[0].close : true);
  const lineColor = $derived(up !== china ? 'var(--green)' : 'var(--red)');

  // Line path across the closes.
  let linePath = $derived.by(() => {
    if (!scale || pts.length < 2) return '';
    return pts
      .map((p, i) => `${i === 0 ? 'M' : 'L'}${scale.xAt(i).toFixed(2)},${scale.yAt(p.close).toFixed(2)}`)
      .join(' ');
  });

  // Sparse date labels under the axis (~6, evenly spread).
  let axisLabels = $derived.by(() => {
    if (!scale) return [];
    const step = Math.max(1, Math.ceil(pts.length / 6));
    const out = [];
    for (let i = 0; i < pts.length; i += step) {
      out.push({ i, x: scale.xAt(i), label: tzDayLabel(pts[i].ts, tz) });
    }
    return out;
  });

  // Marker geometry per point that has observations that day.
  let markers = $derived.by(() => {
    if (!scale || !groups) return [];
    const out = [];
    for (const [i, list] of groups) {
      out.push({ i, x: scale.xAt(i), y: scale.yAt(pts[i].close), n: list.length, obs: list });
    }
    return out;
  });

  let hover = $state(-1);

  function onMove(ev) {
    if (!scale || pts.length < 2) return;
    const rect = ev.currentTarget.getBoundingClientRect();
    const mx = ((ev.clientX - rect.left) / rect.width) * w;
    let best = 0;
    let bestD = Infinity;
    for (let i = 0; i < pts.length; i++) {
      const d = Math.abs(scale.xAt(i) - mx);
      if (d < bestD) {
        bestD = d;
        best = i;
      }
    }
    hover = best;
  }

  let hovered = $derived(pts[hover] ?? null);
  let hoveredObs = $derived(
    markers.find((m) => m.i === hover)?.obs ?? null
  );

  // Tooltip content:
  //  - observation marker → recorded time/price/note (what a marker hover should show)
  //  - ordinary point → the daily (or live) close
  let tipLines = $derived.by(() => {
    if (!hovered) return null;
    if (hoveredObs?.length) {
      const lines = [];
      for (const o of hoveredObs.slice(0, 4)) {
        const when = `${tzDateKey(o.recorded_at, tz).slice(5)} ${tzTimeLabel(o.recorded_at, tz)}`;
        lines.push({
          head: when,
          note: `${fmtQuote(o.price_snapshot, o.currency || currency)}${(o.note ?? '').trim() ? ' · ' + o.note : ''}`,
          isObs: true
        });
      }
      if (hoveredObs.length > 4) {
        lines.push({ head: $t('watchlists.obs.tipMore', { n: hoveredObs.length - 4 }), note: '', isObs: false });
      }
      return lines;
    }
    const price = fmtQuote(hovered.close, currency);
    return [
      {
        head: tzFullDate(hovered.ts, tz),
        note: hovered.live ? $t('watchlists.obs.tipLive', { price }) : $t('watchlists.obs.tipClose', { price }),
        isObs: false
      }
    ];
  });

  // Tooltip placement near the hovered point, kept inside the chart box.
  let tipPos = $derived.by(() => {
    if (!hovered || !scale) return null;
    const x = scale.xAt(hover);
    const y = scale.yAt(hovered.close);
    return { left: x, top: y, flipX: x > w - 190, flipY: y < 88 };
  });
</script>

<div class="chart">
  {#if !scale || pts.length < 2}
    <div class="unavailable"><slot name="empty">—</slot></div>
  {:else}
    <div
      class="plot"
      role="img"
      aria-label={ariaLabel}
      bind:clientWidth={w}
      on:mousemove={onMove}
      on:mouseleave={() => (hover = -1)}
    >
      <svg width={w} height={H}>
        <!-- hover hairline + point -->
        {#if hovered}
          <line x1={scale.xAt(hover)} y1={PAD_TOP} x2={scale.xAt(hover)} y2={H - PAD_BOT} class="hair" />
          <circle cx={scale.xAt(hover)} cy={scale.yAt(hovered.close)} r="3" class="hoverdot" />
        {/if}

        <path d={linePath} fill="none" class="line" style="stroke:{lineColor}" />

        <!-- user observations: amber dots (numbered when several in one day) -->
        {#each markers as m (m.i)}
          {#if m.n > 1}
            <circle cx={m.x} cy={m.y} r="7.5" class="obs-marker" />
            <text x={m.x} y={m.y + 2.8} text-anchor="middle" class="obs-count">{m.n}</text>
          {:else}
            <circle cx={m.x} cy={m.y} r="4" class="obs-marker" />
          {/if}
        {/each}

        <!-- axis labels -->
        {#each axisLabels as a (a.i)}
          <text x={a.x} y={H - 6} text-anchor="middle" class="axis">{a.label}</text>
        {/each}
      </svg>

      {#if tipLines && tipPos}
        <div
          class="tip"
          class:right={tipPos.flipX}
          class:above={tipPos.flipY}
          style="left:{tipPos.left}px;top:{tipPos.top}px"
        >
          {#each tipLines as l, li (li)}
            <div class="tipline" class:obsline={l.isObs}>
              <span class="tiphead">{l.head}</span>
              {#if l.note}<span class="tipnote">{l.note}</span>{/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .chart {
    position: relative;
    min-height: 190px;
  }
  .unavailable {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 190px;
    font-size: var(--text-sm);
    color: var(--muted);
    border: var(--hairline) dashed var(--border);
  }
  .plot {
    position: relative;
    cursor: crosshair;
  }
  svg {
    display: block;
  }
  .line {
    stroke-width: 1.5;
  }
  .hair {
    stroke: var(--border);
    stroke-width: 1;
    stroke-dasharray: 3 3;
  }
  .hoverdot {
    fill: var(--surface);
    stroke: var(--text);
    stroke-width: 1.5;
    pointer-events: none;
  }
  /* Observation markers: amber, clearly distinct from the price line. */
  .obs-marker {
    fill: var(--amber);
    stroke: var(--surface);
    stroke-width: 1.5;
    pointer-events: none;
  }
  .obs-count {
    fill: var(--surface);
    font-size: 8px;
    font-weight: var(--fw-medium);
    font-family: var(--mono);
    pointer-events: none;
    user-select: none;
  }
  .axis {
    fill: var(--dim);
    font-size: 9px;
    font-family: var(--mono);
    pointer-events: none;
  }
  .tip {
    position: absolute;
    z-index: 2;
    transform: translate(10px, -50%);
    background: var(--surface);
    border: var(--hairline) solid var(--border-control);
    box-shadow: var(--shadow-md, 0 4px 14px rgba(0, 0, 0, 0.15));
    padding: var(--space-2) var(--space-3);
    max-width: 280px;
    pointer-events: none;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .tip.right {
    transform: translate(calc(-100% - 10px), -50%);
  }
  .tip.above {
    transform: translate(10px, calc(-100% - 12px));
  }
  .tip.right.above {
    transform: translate(calc(-100% - 10px), calc(-100% - 12px));
  }
  .tipline {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .tiphead {
    font-size: var(--text-xs);
    color: var(--text);
    font-family: var(--mono);
    white-space: nowrap;
  }
  .tipnote {
    font-size: var(--text-xs);
    color: var(--muted);
    line-height: 1.4;
    max-width: 260px;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    overflow-wrap: anywhere;
  }
  .tipline.obsline .tipnote {
    color: var(--text);
  }
</style>
