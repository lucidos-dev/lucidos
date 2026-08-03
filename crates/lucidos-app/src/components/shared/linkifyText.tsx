import type { ComponentChildren } from 'preact';

// Bare http(s) URL matcher. Mirrors the URL pattern in utils/linkifyPaths.ts so
// toast links behave like chat links. `[^\s<>"')\]]+` stops the match at
// whitespace and the closing chars that commonly bracket a URL in prose.
const URL_RE = /https?:\/\/[^\s<>"')\]]+/g;

// Sentence punctuation that follows a URL should stay OUTSIDE the link — a URL
// ending a sentence ("… /docs/getting-started.") must not swallow the period.
const TRAILING_PUNCT = /[.,;:!?]+$/;

/** Turn bare URLs in plain text into clickable links for rendering as JSX.
 *
 *  Unlike `utils/linkifyPaths` (which post-processes an HTML string for chat),
 *  this returns Preact children built from the text directly — the text is never
 *  interpreted as HTML, so it is safe for arbitrary toast content without
 *  `dangerouslySetInnerHTML`. Returns the input string unchanged when it holds
 *  no URL, so the common (no-link) toast allocates nothing extra. */
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
      <a
        key={nodes.length}
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        // The toast message span may carry its own onClick (toast-clickable);
        // opening a link must not also fire it.
        onClick={(e) => e.stopPropagation()}
      >
        {url}
      </a>,
    );
    cursor = end;
  }
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}
