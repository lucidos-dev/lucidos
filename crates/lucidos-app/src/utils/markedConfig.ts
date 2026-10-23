import { marked } from 'marked';
import type { Tokens } from 'marked';
import { widgetEmbedAtStart, type EmbedMatch } from './widgetEmbed';
import { COPY_ICON } from './copyIcons';

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

/** A language name, then only `key=value` or `{...}` attributes:
 *  `python`, `c++`, `js title="a.js" lines=2`, `go {linenos=true}`. */
const LANGUAGE_INFO = /^([\w#+.-]+)(?:\s+(?:[\w-]+=(?:"[^"]*"|'[^']*'|[^\s"']\S*)|\{[^{}]*\}))*$/;

/** The opening fence and its indent, ahead of the info string. */
const OPENING_FENCE = /^ {0,3}(?:`{3,}|~{3,})/;

/** CommonMark names the language by the info string's first word. Any other
 *  info string is code on the opening fence line, as Slack allows. That line
 *  is then the block's first line, taken from `raw`, because marked strips
 *  backslash escapes from `lang`. */
function splitInfoString(info: string, raw: string): { lang: string; firstLine: string } {
  if (!info) return { lang: '', firstLine: '' };
  const language = LANGUAGE_INFO.exec(info)?.[1];
  return language
    ? { lang: language, firstLine: '' }
    : { lang: '', firstLine: raw.split('\n', 1)[0].replace(OPENING_FENCE, '').trim() };
}

const renderer = new marked.Renderer();
renderer.code = ({ text, lang: info = '', escaped, raw }: Tokens.Code) => {
  const { lang, firstLine } = splitInfoString(info, raw);
  const langLabel = lang ? `<span class="code-block-lang">${escapeHtmlAttr(lang)}</span>` : '';
  const body = [escapeHtmlAttr(firstLine), escaped ? text : escapeHtmlAttr(text)].filter(Boolean).join('\n');
  // Lang label + copy button share one .code-block-header overlay. See
  // `.code-block-header` in styles/chat/response.css for why this structure
  // (not two absolute siblings of <pre>).
  return `<div class="code-block-wrapper" ${CODE_COPY_ATTR}="${COPY_ID_NONCE}"><div class="code-block-header">${langLabel}<button type="button" class="copy-btn code-block-copy-btn" aria-label="Copy code">${COPY_ICON}</button></div><pre><code>${body}</code></pre></div>`;
};

/** Marks a GFM task-list checkbox this renderer wrote, carrying the same
 *  nonce as the copy markers above. Raw HTML in markdown passes through
 *  unchanged, and DOMPurify keeps `data-*` by default. So an authored
 *  `<input type="checkbox">` is otherwise indistinguishable in the DOM from
 *  one `marked` generated for a real `- [ ]` line.
 *  `MarkdownDocument`'s checkbox-toggle wiring only counts the latter when
 *  mapping a click back to a source line (`taskListToggle.ts`). Querying
 *  every checkbox would pick up the authored one too, shifting every later
 *  index and toggling the wrong line. */
export const TASK_CHECKBOX_ATTR = 'data-task-checkbox';

renderer.checkbox = ({ checked }: Tokens.Checkbox): string =>
  `<input ${TASK_CHECKBOX_ATTR}="${COPY_ID_NONCE}"${checked ? ' checked' : ''} disabled type="checkbox"> `;

/** Marks an email the GFM autolinker found in bare text, so `renderMarkdown`
 *  can unwrap one sitting inside a path. Its token's `raw` is the bare address;
 *  an authored `<x@y>` or `[x](mailto:x)` keeps its brackets in `raw`. */
export const BARE_EMAIL_ATTR = 'data-bare-email';

renderer.link = function (token: Tokens.Link): string {
  const html = marked.Renderer.prototype.link.call(this, token);
  const bareEmail = token.href.startsWith('mailto:') && token.raw === token.text;
  return bareEmail ? html.replace(/^<a /, `<a ${BARE_EMAIL_ATTR} `) : html;
};

marked.setOptions({
  breaks: true,
  gfm: true,
  renderer,
});

/** Marks a *widget embed* this renderer wrote (ADR 0415), carrying the copy
 *  nonce, so authored HTML cannot forge a slot. The slot holds the embed's
 *  label until `useWidgetEmbedSlots` mounts its widget, which happens only in
 *  a reply's final text: a streaming reply shows the label. */
export const WIDGET_EMBED_SLOT_ATTR = 'data-widget-embed-slot';
/** The slot's embed as JSON, or absent when its syntax is broken. */
export const WIDGET_EMBED_ATTR = 'data-widget-embed';

marked.use({
  extensions: [{
    name: 'widgetEmbed',
    level: 'inline',
    start(src: string) {
      const at = src.indexOf('![');
      return at < 0 ? undefined : at;
    },
    tokenizer(src: string) {
      const match = widgetEmbedAtStart(src);
      // A broken embed that never closes is left as text, so it cannot
      // swallow the rest of its paragraph.
      if (!match || (!match.embed.ok && src[match.end - 1] !== ')')) return undefined;
      return { type: 'widgetEmbed', raw: src.slice(0, match.end), embed: match.embed };
    },
    renderer(token) {
      const { embed } = token as unknown as { embed: EmbedMatch['embed'] };
      const label = embed.ok ? embed.value.label ?? '' : '';
      const data = embed.ok ? ` ${WIDGET_EMBED_ATTR}="${escapeHtmlAttr(JSON.stringify(embed.value))}"` : '';
      return `<span class="widget-embed-slot" ${WIDGET_EMBED_SLOT_ATTR}="${COPY_ID_NONCE}"${data}>${escapeHtmlAttr(label)}</span>`;
    },
  }],
});
