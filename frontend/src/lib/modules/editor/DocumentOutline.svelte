<script>
  // Generic document outline — a lightweight table of contents rendered from derived
  // heading entries. It knows nothing about the editor, the scroll container or the
  // document format: the host passes a flat list of { level, text } entries plus the
  // index of the currently-read section and a callback to jump to an entry.
  //
  // Visual rules (spec-driven):
  //   • hierarchy is shown by indentation per level — no tree structure required
  //   • an entry is a real <button>, so mouse, keyboard focus and Enter all work
  //   • titles truncate to one line, so a long heading can never break the layout
  //   • the list scrolls on its own when it outgrows the space the host allots.
  let {
    items = [], // [{ level, text }] in document order
    active = -1, // index of the current section, -1 = none
    onpick = () => {}, // (index) → jump to that heading
    label = '' // accessible name / visible title of the outline
  } = $props();

  let scrollEl;
  let listEl;

  // Keep the active entry visible inside the outline's own scroll area (no-op when
  // the whole list already fits). Direct scrollTop math — never scrollIntoView —
  // so this can never scroll the editor's document.
  $effect(() => {
    const holder = scrollEl;
    if (!holder || active < 0 || !listEl) return;
    const el = listEl.querySelector(`[data-idx="${active}"]`);
    if (!el) return;
    const top = el.offsetTop;
    const bottom = top + el.offsetHeight;
    if (top < holder.scrollTop) {
      holder.scrollTop = Math.max(0, top - 4);
    } else if (bottom > holder.scrollTop + holder.clientHeight) {
      holder.scrollTop = Math.max(0, bottom - holder.clientHeight + 4);
    }
  });
</script>

<nav class="doc-outline" aria-label={label}>
  <div class="do-head">{label}</div>
  <div class="do-scroll" bind:this={scrollEl}>
    <ul class="do-list" bind:this={listEl}>
      {#each items as item, i (i)}
        <li>
          <button
            type="button"
            class="do-item lv-{item.level}"
            class:active={i === active}
            data-idx={i}
            title={item.text}
            aria-current={i === active ? 'location' : undefined}
            onclick={() => onpick(i)}
          >
            <span class="do-text">{item.text}</span>
          </button>
        </li>
      {/each}
    </ul>
  </div>
</nav>

<style>
  .doc-outline {
    display: flex;
    flex-direction: column;
    min-width: 0;
    width: 100%;
  }
  .do-head {
    flex: none;
    padding: 0 6px 6px;
    color: var(--muted);
    font-size: var(--text-xs);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* The host decides how tall the outline may be via --outline-cap. */
  .do-scroll {
    position: relative;
    min-width: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    max-height: var(--outline-cap, 50vh);
    scrollbar-width: thin;
  }
  .do-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
  }
  .do-item {
    display: block;
    width: 100%;
    max-width: 100%;
    border: none;
    background: transparent;
    color: var(--muted);
    font-family: inherit;
    font-size: var(--text-sm);
    line-height: 1.35;
    text-align: left;
    cursor: pointer;
    padding: 3px 8px;
    border-radius: var(--radius);
    /* One line only — a very long title is truncated, never allowed to stretch
       the outline or push its siblings around. */
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .do-item:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  .do-item:focus-visible {
    outline: 1px solid var(--border-control);
    outline-offset: -1px;
  }
  .do-item.active {
    color: var(--text);
    background: var(--surface-2);
    font-weight: var(--fw-medium);
  }
  /* Hierarchy via indentation only (H1 / H2 / H3). Non-standard level order in
     the document (H1 → H3 → H2) still renders fine — each entry is indented by
     its own level. */
  .lv-1 {
    padding-left: 8px;
  }
  .lv-2 {
    padding-left: 24px;
  }
  .lv-3 {
    padding-left: 40px;
  }
</style>
