import { renderMarkdown } from '../../utils/renderMarkdown';

const REMOTE_ELEMENTS =
  'img, picture, source, video, audio, iframe, object, embed, svg, link, style, input, button, form, select, textarea';

const REMOTE_ATTRIBUTES = ['style', 'src', 'srcset', 'background', 'poster', 'action', 'formaction'];

const OUTBOUND_LINK = /^(https?:|mailto:)/i;

export function renderPluginReadme(markdown: string): string {
  const doc = new DOMParser().parseFromString(`<body>${renderMarkdown(markdown)}</body>`, 'text/html');
  doc.body.querySelectorAll(REMOTE_ELEMENTS).forEach((el) => el.remove());
  doc.body.querySelectorAll('*').forEach((el) => {
    for (const name of REMOTE_ATTRIBUTES) el.removeAttribute(name);
  });
  doc.body.querySelectorAll('a').forEach((a) => {
    const href = a.getAttribute('href') ?? '';
    if (OUTBOUND_LINK.test(href)) {
      a.setAttribute('target', '_blank');
      a.setAttribute('rel', 'noopener noreferrer');
    } else {
      a.removeAttribute('href');
    }
  });
  return doc.body.innerHTML;
}
