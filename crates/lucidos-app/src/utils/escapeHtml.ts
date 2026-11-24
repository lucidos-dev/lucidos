const el = document.createElement('div');

export function escapeHtml(text: string): string {
  el.textContent = text;
  return el.innerHTML;
}

let inertBody: HTMLElement | null | undefined;

export function makeInertBody(): HTMLElement | null {
  const impl = document.implementation as DOMImplementation | undefined;
  return typeof impl?.createHTMLDocument === 'function'
    ? impl.createHTMLDocument('').body
    : null;
}

function getInertBody(): HTMLElement | null {
  if (inertBody === undefined) inertBody = makeInertBody();
  return inertBody;
}

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
