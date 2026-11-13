import { marked } from 'marked';
import type { Tokens } from 'marked';
import { widgetEmbedAtStart, type EmbedMatch } from './widgetEmbed';
import { escapeHtmlAttr } from './escapeHtmlAttr';
import {
  BARE_EMAIL_ATTR, CODE_COPY_ATTR, COPY_ID_NONCE, WIDGET_EMBED_ATTR, WIDGET_EMBED_ERROR_ATTR, WIDGET_EMBED_SLOT_ATTR,
  taskCheckboxTag,
} from './markdownMarkers';

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
  // (not two absolute siblings of <pre>). The sanitizer escapes every
  // `<button>`, so `resolveCodeBlockCopy` adds the copy button after it.
  return `<div class="code-block-wrapper" ${CODE_COPY_ATTR}="${COPY_ID_NONCE}"><div class="code-block-header">${langLabel}</div><pre><code>${body}</code></pre></div>`;
};

renderer.checkbox = ({ checked }: Tokens.Checkbox): string => `${taskCheckboxTag(checked)} `;

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
      const data = embed.ok
        ? ` ${WIDGET_EMBED_ATTR}="${escapeHtmlAttr(JSON.stringify(embed.value))}"`
        : ` ${WIDGET_EMBED_ERROR_ATTR}="${escapeHtmlAttr(embed.reason)}"`;
      return `<span class="widget-embed-slot" ${WIDGET_EMBED_SLOT_ATTR}="${COPY_ID_NONCE}"${data}>${escapeHtmlAttr(label)}</span>`;
    },
  }],
});
