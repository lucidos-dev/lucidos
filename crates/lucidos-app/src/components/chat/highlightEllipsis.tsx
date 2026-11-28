import type { ComponentChildren } from 'preact';

export function highlightEllipsis(text: string): ComponentChildren {
  if (!text.includes('…') && !text.endsWith('...')) return text;

  const out: ComponentChildren[] = [];
  let key = 0;
  const segments = text.split('…');
  for (let i = 0; i < segments.length; i++) {
    if (i > 0) out.push(<span key={key++} class="ellipsis-marker">…</span>);
    const seg = segments[i];
    const isLast = i === segments.length - 1;
    if (isLast && seg.endsWith('...')) {
      if (seg.length > 3) out.push(seg.slice(0, -3));
      out.push(<span key={key++} class="ellipsis-marker">...</span>);
    } else if (seg.length > 0) {
      out.push(seg);
    }
  }
  return out;
}
