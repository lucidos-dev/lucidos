const el = document.createElement('div');

export function escapeHtml(text: string): string {
  el.textContent = text;
  return el.innerHTML;
}

/** The plain text of an HTML fragment, parsed in an INERT document.
 *
 *  Never in this one. A detached element still belongs to the live document. So
 *  `<img src=x onerror=…>` in its `innerHTML` creates a real image and runs the
 *  handler. Callers pass HTML nothing upstream sanitized (a slides deck's
 *  subtitle), so the ordinary element made this a script-execution sink.
 *
 *  `createHTMLDocument` returns a document with no browsing context. It runs no
 *  script and loads no resource, which is what makes the same assignment safe
 *  there.
 *
 *  The fallback is for the unit-test environment alone, whose hand-rolled
 *  `document` stub (src/test-setup.ts) has no `implementation`. Every browser
 *  Lucidos ships on has carried `createHTMLDocument` for over a decade, so no
 *  real client takes that branch. It parses nothing either way, so neither
 *  branch is a sink and neither touches `el`. */
let inertBody: HTMLElement | null | undefined;

/** The inert document's body, made once. Both callers run per list item per
 *  render, so building a whole document per call would cost N of them a frame.
 *  `undefined` means not yet probed, `null` means this environment has no
 *  `createHTMLDocument`. */
function getInertBody(): HTMLElement | null {
  if (inertBody === undefined) {
    const impl = document.implementation as DOMImplementation | undefined;
    inertBody = typeof impl?.createHTMLDocument === 'function'
      ? impl.createHTMLDocument('').body
      : null;
  }
  return inertBody;
}

/** The same extraction as pure string work, for an environment with no
 *  `createHTMLDocument`. Assigning to `el` was the old fallback, and that put
 *  untrusted markup back into the live document by the other door. Nothing
 *  here parses, so nothing here can run.
 *
 *  `&amp;` is decoded last, so `&amp;lt;` yields `&lt;` and not `<`. */
function stripTagsAsText(html: string): string {
  return html
    .replace(/<[^>]*>/g, '')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&nbsp;/g, ' ')
    .replace(/&amp;/g, '&');
}

export function stripHtml(html: string): string {
  const body = getInertBody();
  if (!body) return stripTagsAsText(html);
  body.innerHTML = html;
  return body.textContent ?? '';
}
