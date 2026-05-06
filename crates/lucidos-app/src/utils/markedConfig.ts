import { marked } from 'marked';
import type { Tokens } from 'marked';

const COPY_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M3 10.5V3a1.5 1.5 0 0 1 1.5-1.5H10"/></svg>';
const CHECK_ICON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3.5 8.5 6.5 11.5 12.5 4.5"/></svg>';

export function escapeHtmlAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

const renderer = new marked.Renderer();
renderer.code = ({ text, lang, escaped }: Tokens.Code) => {
  const langLabel = lang ? `<span class="code-block-lang">${escapeHtmlAttr(lang)}</span>` : '';
  const body = escaped ? text : escapeHtmlAttr(text);
  return `<div class="code-block-wrapper">${langLabel}<button type="button" class="copy-btn code-block-copy-btn" aria-label="Copy code">${COPY_ICON}</button><pre><code>${body}</code></pre></div>`;
};

marked.setOptions({
  breaks: true,
  gfm: true,
  renderer,
});

export { COPY_ICON, CHECK_ICON };
