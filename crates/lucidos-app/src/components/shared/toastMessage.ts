/** A parsed restart-required (or similar) toast message:
 *  the first line is the heading, blank lines start a new section, and lines
 *  prefixed with "• " are bullets within their section. */
export interface ParsedToastMessage {
  heading: string;
  sections: Array<{ title?: string; bullets: string[] }>;
}

const BULLET_PREFIX = '• ';

/** Serialize a notification's separate `title` and `body` into the single
 *  message string `parseToastMessage` reads back — the inverse of the parse
 *  contract above, so keep the two in step.
 *
 *  A body that carries its own structure — more than one line, or a leading
 *  bullet — MUST begin on line 2 so the title owns the heading. Joining them
 *  with ": " instead folds the body's first line into the heading: with a
 *  bulleted body that silently ate the first bullet, and with a body whose
 *  lead line restated the title it rendered the doubled heading
 *  "1 change ready to apply: 1 change ready to apply". A ONE-item bulleted
 *  body counts as structured too — it has no newline, so testing only for one
 *  would inline it as "…apply: • Set Opus 5 as Default".
 *
 *  A single-line body with no bullet has no structure to protect and reads
 *  better as a continuation of the title ("Permission needed: Edit /path"),
 *  so it stays inline — pushing it to line 2 would render it as a bold
 *  section title under a plain heading, which inverts the visual weight. */
export function composeToastMessage(title: string, body: string): string {
  if (!body) return title;
  const structured = body.includes('\n') || body.startsWith(BULLET_PREFIX);
  return structured ? `${title}\n${body}` : `${title}: ${body}`;
}

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
