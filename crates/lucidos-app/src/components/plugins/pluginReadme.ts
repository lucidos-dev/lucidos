import { renderMarkdown } from '../../utils/renderMarkdown';

/** Elements a README may not keep. The renderer's sanitizer already strips
 *  script and event handlers (invariant I7). These go too, because each one
 *  fetches from a host the plugin's author picked, or draws a control, before
 *  the user installed anything. The author lists pictures as `screenshots`. */
const REMOTE_ELEMENTS =
  'img, picture, source, video, audio, iframe, object, embed, svg, link, style, input, button, form, select, textarea';

/** Attributes that fetch a resource or restyle the page. */
const REMOTE_ATTRIBUTES = ['style', 'src', 'srcset', 'background', 'poster', 'action', 'formaction'];

/** A link may leave for the web or for mail, never into this workspace. */
const OUTBOUND_LINK = /^(https?:|mailto:)/i;

/** A plugin's `media/README.md` as safe HTML for the plugin detail page. Its
 *  links open in a new tab: the README describes the plugin, and a relative
 *  link would otherwise resolve against the user's own workspace. */
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
