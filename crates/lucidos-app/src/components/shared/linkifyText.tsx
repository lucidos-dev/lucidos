import type { ComponentChildren } from 'preact';

// Bare http(s) URL matcher. Mirrors the URL pattern in utils/linkifyPaths.ts so
// toast links behave like chat links. `[^\s<>"')\]]+` stops the match at
// whitespace and the closing chars that commonly bracket a URL in prose.
const URL_RE = /https?:\/\/[^\s<>"')\]]+/g;

// Sentence punctuation that follows a URL should stay OUTSIDE the link — a URL
// ending a sentence ("… /docs/getting-started.") must not swallow the period.
const TRAILING_PUNCT = /[.,;:!?]+$/;

/** Turn bare URLs in plain text into clickable links, as JSX.
 *
 *  `utils/linkifyPaths` post-processes an HTML string for chat. This builds
 *  Preact children from the text instead, so nothing is ever interpreted as
 *  HTML and no caller needs `dangerouslySetInnerHTML`. Text with no URL comes
 *  back unchanged, so a link-free caller allocates nothing.
 *
 *  The anchor carries NO click handler, deliberately. `onGlobalClick`
 *  (hooks/useStartup.ts) claims every absolute http(s) anchor and routes it
 *  through `openUrl`. That is where the in-app browser, the packaged client's
 *  OS opener and the iOS PWA Safari hand-off live.
 *
 *  A `stopPropagation` here would keep the click off that funnel. WKWebView
 *  drops a bare `_blank` navigation, so the link would then be dead in the
 *  packaged app. A clickable ANCESTOR skips its own action instead, as `Toast`
 *  does. */
export function linkifyText(text: string): ComponentChildren {
  const matches = [...text.matchAll(URL_RE)];
  if (matches.length === 0) return text;

  const nodes: ComponentChildren[] = [];
  let cursor = 0;
  for (const m of matches) {
    const start = m.index;
    let url = m[0];
    let end = start + url.length;
    // Peel trailing sentence punctuation back out of the link so it renders as
    // plain text after the anchor.
    const trailing = url.match(TRAILING_PUNCT);
    if (trailing) {
      url = url.slice(0, url.length - trailing[0].length);
      end -= trailing[0].length;
    }
    if (!url) continue;
    if (start > cursor) nodes.push(text.slice(cursor, start));
    nodes.push(
      // `accent-link` is the app's link appearance, so a surface adopting this
      // helper needs no stylesheet rule of its own.
      <a
        key={nodes.length}
        class="accent-link"
        href={url}
        target="_blank"
        rel="noopener noreferrer"
      >
        {url}
      </a>,
    );
    cursor = end;
  }
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}
