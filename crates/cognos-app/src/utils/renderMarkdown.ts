import { marked } from 'marked';

// SVG icons for copy button states
const COPY_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M3 10.5V3a1.5 1.5 0 0 1 1.5-1.5H10"/></svg>';
const CHECK_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3.5 8.5 6.5 11.5 12.5 4.5"/></svg>';

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

const renderer = new marked.Renderer();
renderer.code = ({ text, lang }: { text: string; lang?: string }) => {
  // text is pre-escaped by marked; lang is NOT — must escape to prevent XSS
  const langLabel = lang ? `<span class="code-block-lang">${escapeHtml(lang)}</span>` : '';
  return `<div class="code-block-wrapper">${langLabel}<button type="button" class="copy-btn code-block-copy-btn" aria-label="Copy code">${COPY_ICON}</button><pre><code>${text}</code></pre></div>`;
};

// Configure marked for safe rendering
marked.setOptions({
  breaks: true,   // Convert \n to <br>
  gfm: true,      // GitHub-flavored markdown
  renderer,
});

// Unique marker prefix for copy block boundaries (survives marked processing)
const COPY_MARKER = 'COGNOS_COPY_BLOCK';
const COPY_MARKER_PATTERN = new RegExp(
  `<!--${COPY_MARKER}_START_(\\d+)-->([\\s\\S]*?)<!--${COPY_MARKER}_END_\\1-->`,
  'g',
);

// Matches fenced code blocks and inline code spans for protection during copy block preprocessing
const CODE_PROTECTION_PATTERN = /```[\s\S]*?```|`[^`\n]+`/g;

/**
 * Convert <copy>...</copy> tags to copyable UI blocks.
 *
 * Inline (single-line): wraps directly in a <span> before marked — works fine
 * because there are no blank lines to break the HTML block.
 *
 * Multiline: uses HTML comment markers that survive marked processing, then
 * postprocessCopyBlocks wraps the rendered content. This avoids CommonMark's
 * HTML block rule that ends a <div> at the first blank line.
 */
function preprocessCopyBlocks(md: string, encodedTexts: Map<number, string>): string {
  // Protect fenced code blocks and inline code spans from copy tag matching.
  // Replace them with placeholders, process copy blocks, then restore.
  const codeSlots: string[] = [];
  let safeMd = md.replace(CODE_PROTECTION_PATTERN, (match) => {
    const idx = codeSlots.length;
    codeSlots.push(match);
    return `\x00CODE${idx}\x00`;
  });

  let counter = 0;
  safeMd = safeMd.replace(/<copy>([\s\S]*?)<\/copy>/g, (_match, content: string) => {
    const trimmed = content.trim();
    const encoded = trimmed
      .replace(/&/g, '&amp;')
      .replace(/"/g, '&quot;')
      .replace(/\n/g, '&#10;');

    const isMultiline = trimmed.includes('\n');

    // Restore any protected code spans in the copy text so backticks are preserved
    const restoredEncoded = encoded.replace(/\x00CODE(\d+)\x00/g, (_, idx) => {
      const original = codeSlots[parseInt(idx, 10)];
      return original.replace(/&/g, '&amp;').replace(/"/g, '&quot;');
    });

    if (!isMultiline) {
      return `<span class="copyable-block" data-copy-text="${restoredEncoded}">` +
        trimmed +
        `<button type="button" class="copy-btn" aria-label="Copy to clipboard">${COPY_ICON}</button>` +
        `</span>`;
    }

    // Multiline: stash encoded text, emit comment markers around raw markdown.
    const id = counter++;
    encodedTexts.set(id, restoredEncoded);
    return `<!--${COPY_MARKER}_START_${id}-->\n\n${trimmed}\n\n<!--${COPY_MARKER}_END_${id}-->`;
  });

  // Restore protected code blocks
  return safeMd.replace(/\x00CODE(\d+)\x00/g, (_, idx) => codeSlots[parseInt(idx, 10)]);
}

function postprocessCopyBlocks(html: string, encodedTexts: Map<number, string>): string {
  return html.replace(COPY_MARKER_PATTERN, (_match, idStr: string, inner: string) => {
    const id = parseInt(idStr, 10);
    const encoded = encodedTexts.get(id) ?? '';
    return `<div class="copyable-block copyable-block-multi" data-copy-text="${encoded}">` +
      inner.trim() +
      `<button type="button" class="copy-btn" aria-label="Copy to clipboard">${COPY_ICON}</button>` +
      `</div>`;
  });
}

// Escape dangerous HTML elements that survive marked processing.
// Raw HTML in markdown source (e.g., user typing "<iframe>") passes through
// marked unescaped and renders as actual elements — blank boxes, XSS vectors, etc.
// Copy block elements (<span>, <button>, <svg>, <div>) and marked-generated
// elements (<p>, <strong>, <code>, etc.) are NOT in this list and pass through.
const DANGEROUS_TAG = /<(\/?)(iframe|script|style|object|embed|applet|base|meta|link)(\s[^>]*)?>/gi;

export function renderMarkdown(md: string): string {
  // Preprocess copy blocks before marked parsing
  const encodedTexts = new Map<number, string>();
  const preprocessed = preprocessCopyBlocks(md, encodedTexts);
  let html = marked.parse(preprocessed, { async: false }) as string;
  // Wrap multiline copy blocks that used comment markers
  html = postprocessCopyBlocks(html, encodedTexts);
  // Escape dangerous HTML elements from raw markdown source
  html = html.replace(DANGEROUS_TAG, (match) => escapeHtml(match));
  // Wrap tables in a scrollable container so columns auto-size naturally
  html = html.replace(/<table>/g, '<div class="table-scroll-wrapper"><table>');
  html = html.replace(/<\/table>/g, '</table></div>');
  // Convert thread: links to clickable thread navigation
  html = html.replace(
    /href="thread:([0-9a-f-]+)"/g,
    'href="#" data-thread-id="$1" class="thread-link"'
  );
  return html;
}

export { COPY_ICON, CHECK_ICON };
