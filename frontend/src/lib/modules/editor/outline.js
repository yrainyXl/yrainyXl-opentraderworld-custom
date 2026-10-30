/**
 * Document outline derivation — the "Rich Text Document → Outline" step.
 *
 * The rich-text document (ProseMirror JSON, the SSOT) is walked ONCE for semantic
 * `heading` nodes. The outline is always derived data: nothing here reads, writes or
 * knows about storage, and the document is never modified for the outline's sake.
 *
 * Keeping this module UI-free means the same traversal can later feed other derived
 * views (Markdown export, AI context serialization…) without an editor dependency.
 */

/** Heading levels the outline recognises. Level 4+ is ignored. */
export const OUTLINE_LEVELS = [1, 2, 3];

/**
 * Flatten a ProseMirror node subtree to its plain text. Marks (bold, italic, …) are
 * deliberately ignored — only the characters matter for an outline entry.
 */
function nodeText(node) {
  if (!node) return '';
  if (node.type === 'text') return node.text ?? '';
  const kids = node.content;
  if (!kids) return '';
  let out = '';
  for (const child of kids) out += nodeText(child);
  return out;
}

/**
 * Walk the document in order and collect the semantic headings that make up the
 * outline: `{ level, text }` where level ∈ OUTLINE_LEVELS and text is non-empty.
 *
 * `doc` is ProseMirror JSON (`{ type: 'doc', content: [...] }`); any nested block
 * (blockquote, list item, table cell, …) is traversed too, so headings that live
 * inside a container still appear in document order.
 *
 * Empty headings are skipped, and ordinary bold/large text is never treated as a
 * heading — only real `heading` nodes with a supported level count.
 */
export function collectHeadings(doc) {
  const entries = [];
  if (!doc || !Array.isArray(doc.content)) return entries;

  const walk = (node) => {
    if (!node || !node.type) return;
    if (node.type === 'heading') {
      const level = Number(node.attrs?.level) || 1;
      if (OUTLINE_LEVELS.includes(level)) {
        const text = nodeText(node).trim();
        if (text) entries.push({ level, text });
      }
      // A heading's inline content can only hold text/marks — nothing to descend into.
      return;
    }
    const kids = node.content;
    if (kids) for (const child of kids) walk(child);
  };

  for (const child of doc.content) walk(child);
  return entries;
}
