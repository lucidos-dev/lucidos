import type { ComponentChildren } from 'preact';

const URL_RE = /https?:\/\/[^\s<>"')\]]+/g;

const TRAILING_PUNCT = /[.,;:!?]+$/;

export function linkifyText(text: string): ComponentChildren {
  const matches = [...text.matchAll(URL_RE)];
  if (matches.length === 0) return text;

  const nodes: ComponentChildren[] = [];
  let cursor = 0;
  for (const m of matches) {
    const start = m.index;
    let url = m[0];
    let end = start + url.length;
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
