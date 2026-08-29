import { clampText } from '../../utils/clampText';
import type { ToastType } from '../../store/types';

/** A parsed restart-required (or similar) toast message:
 *  the first line is the heading, blank lines start a new section, and lines
 *  prefixed with "• " are bullets within their section. */
export interface ParsedToastMessage {
  heading: string;
  sections: Array<{ title?: string; bullets: string[] }>;
}

const BULLET_PREFIX = '• ';

/** Longest ERROR message, in characters. An error is one sentence, and the
 *  longest the engine writes is around 180. A 390pt phone shows about 150 in
 *  the heading's six-line box, so a message at this budget is one short scroll
 *  rather than a page. No count can promise "always fits": the same string
 *  wraps to more lines on a 320pt screen than on a 430pt one. */
const ERROR_MAX_CHARS = 200;

/** Longest message of any other kind. A status or notification toast may
 *  legitimately carry a titled list, such as the engine build's commit groups.
 *  So this is a backstop against a pathological payload, not a style rule. An
 *  app reaching `showToast` through the toast bridge is bounded here too. */
const TOAST_MAX_CHARS = 2000;

/** What `showToast` stores, rather than what the caller passed.
 *
 *  An ERROR is flattened to one line as well as clamped. `parseToastMessage`
 *  below reads structure out of newlines, so a body carrying them renders as a
 *  bold title over a bulleted list. Right for a build's commit groups, wrong
 *  for a failure. It is how an HTML holding page once rendered as a list of its
 *  own `<meta>` tags. */
export function clampToastMessage(message: string, type: ToastType): string {
  if (type !== 'error') return clampText(message, TOAST_MAX_CHARS);
  return clampText(message.replace(/\s+/g, ' ').trim(), ERROR_MAX_CHARS);
}

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
