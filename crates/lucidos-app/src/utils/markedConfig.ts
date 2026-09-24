import { marked } from 'marked';
import type { Tokens } from 'marked';

const COPY_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M3 10.5V3a1.5 1.5 0 0 1 1.5-1.5H10"/></svg>';
const CHECK_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3.5 8.5 6.5 11.5 12.5 4.5"/></svg>';

export function escapeHtmlAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

/** Unguessable, per module load, so content cannot name a copy slot.
 *
 *  Content authors both halves of a copy forgery: a real block hidden where
 *  nobody reads it, and a decoy label carrying that block's slot. Every marker
 *  the renderer writes carries this nonce, so content has no slot to name.
 *  `renderMarkdown.ts` states where each marker is resolved. */
export const COPY_ID_NONCE = ((): string => {
  const bytes = new Uint8Array(8);
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes);
  } else {
    for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
  }
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
})();

/** Marks a code block this renderer wrote, so its Copy button gets a payload. */
export const CODE_COPY_ATTR = 'data-code-copy';

const renderer = new marked.Renderer();
renderer.code = ({ text, lang, escaped }: Tokens.Code) => {
  const langLabel = lang ? `<span class="code-block-lang">${escapeHtmlAttr(lang)}</span>` : '';
  const body = escaped ? text : escapeHtmlAttr(text);
  // Lang label + copy button share one .code-block-header overlay — see
  // chat.css `.code-block-header` for why this structure (not two absolute
  // siblings of <pre>).
  return `<div class="code-block-wrapper" ${CODE_COPY_ATTR}="${COPY_ID_NONCE}"><div class="code-block-header">${langLabel}<button type="button" class="copy-btn code-block-copy-btn" aria-label="Copy code">${COPY_ICON}</button></div><pre><code>${body}</code></pre></div>`;
};

marked.setOptions({
  breaks: true,
  gfm: true,
  renderer,
});

export { COPY_ICON, CHECK_ICON };
