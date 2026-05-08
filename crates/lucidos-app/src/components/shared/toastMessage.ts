/** A parsed restart-required (or similar) toast message:
 *  the first line is the heading, blank lines start a new section, and lines
 *  prefixed with "• " are bullets within their section. */
export interface ParsedToastMessage {
  heading: string;
  sections: Array<{ title?: string; bullets: string[] }>;
}

const BULLET_PREFIX = '• ';

export function parseToastMessage(message: string): ParsedToastMessage {
  const lines = message.split('\n');
  const heading = lines[0] ?? '';
  const sections: ParsedToastMessage['sections'] = [];
  let current: { title?: string; bullets: string[] } | null = null;

  for (let i = 1; i < lines.length; i++) {
    const line = lines[i];
    if (line === '') {
      if (current) {
        sections.push(current);
        current = null;
      }
      continue;
    }
    if (!current) current = { bullets: [] };
    if (line.startsWith(BULLET_PREFIX)) {
      current.bullets.push(line.slice(BULLET_PREFIX.length));
    } else if (current.title === undefined) {
      current.title = line;
    } else {
      current.bullets.push(line);
    }
  }
  if (current) sections.push(current);
  return { heading, sections };
}
