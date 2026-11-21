import { frontmatterLineCount } from './frontmatter';

/** A GFM task-list item marker: a bullet or ordered marker, then `[ ]` /
 *  `[x]` / `[X]`. A space, a tab, or the line's end must follow the closing
 *  `]` (an optional trailing `\r` allowed before that end). Group 1 captures
 *  the prefix up to and including the opening `[`; group 2 captures the
 *  status char. The closing `]` is matched but not captured, so a rewrite
 *  replaces group 2 and re-inserts the `]` literally. */
const TASK_ITEM = /^(\s*(?:[-*+]|\d{1,9}[.)])\s+\[)([ xX])\](?=[ \t]|\r?$)/;

/** A fence delimiter line: up to 3 leading spaces, then 3+ of the same fence
 *  character. `rest` is everything after the run, trimmed; a closing fence
 *  must have nothing else on the line. */
const FENCE_LINE = /^ {0,3}(`{3,}|~{3,})(.*)$/;

/** One or more `>` blockquote markers leading a line, each optionally
 *  followed by one space or tab (`> > - [ ] nested`). A quoted list's task
 *  items render in the same source order as everything else, so stripping
 *  this prefix before matching counts them correctly. The stripped text is
 *  spliced back in unchanged on a rewrite. Lazy continuation (a quoted line
 *  with the `>` omitted) is not recognized, matching this module's
 *  line-by-line scan rather than a full block-structure parse. */
const BLOCKQUOTE_PREFIX = /^(?: {0,3}>[ \t]?)+/;

interface FenceMatch { char: string; len: number; rest: string }

function matchFence(line: string): FenceMatch | null {
  const m = FENCE_LINE.exec(line);
  if (!m) return null;
  return { char: m[1][0], len: m[1].length, rest: m[2].trim() };
}

interface TaskItemLine {
  /** Index into the `'\n'`-split line array. */
  index: number;
  /** Length of this line's leading blockquote prefix, to splice back in
   *  untouched on a rewrite. */
  quoteLen: number;
  /** The line with its blockquote prefix removed. */
  rest: string;
  /** The `TASK_ITEM` match against `rest`. */
  match: RegExpExecArray;
}

/** Yields every GFM task-list item line in `fullFileContent`, top to bottom,
 *  in the exact order `toggleTaskListCheckbox` and `countTaskListItems` both
 *  rely on. One shared scan, so the two can never disagree.
 *
 *  Skips the frontmatter block (the same detection `splitFrontmatter` uses,
 *  via `frontmatterLineCount`) and a fenced code block's contents (3+
 *  backticks or tildes, matching close). A leading blockquote prefix is
 *  stripped before matching either, since `marked` renders a quoted list's
 *  checkboxes too. */
function* taskItemLines(fullFileContent: string): Generator<TaskItemLine> {
  const bodyStart = frontmatterLineCount(fullFileContent);
  const lines = fullFileContent.split('\n');
  let fence: { char: string; len: number } | null = null;

  for (let i = bodyStart; i < lines.length; i++) {
    const line = lines[i];
    const quoteLen = BLOCKQUOTE_PREFIX.exec(line)?.[0].length ?? 0;
    const rest = line.slice(quoteLen);

    if (fence) {
      const closing = matchFence(rest);
      if (closing && closing.char === fence.char && closing.len >= fence.len && closing.rest === '') {
        fence = null;
      }
      continue; // Every line inside a fence, including its closer, is never a task item.
    }

    const opening = matchFence(rest);
    if (opening) {
      fence = { char: opening.char, len: opening.len };
      continue; // An opening fence line is never a task item either.
    }

    const match = TASK_ITEM.exec(rest);
    if (!match) continue;
    yield { index: i, quoteLen, rest, match };
  }
}

/** How many GFM task-list items `toggleTaskListCheckbox` would find in
 *  `fullFileContent`.
 *
 *  A caller that also knows the RENDERED checkbox count (from the DOM) can
 *  compare the two. A mismatch means some construct in this file (an
 *  indented code block, for one) makes index-based mapping unreliable here.
 *  Checkbox interactivity should then fail closed, rather than risk
 *  toggling the wrong line. */
export function countTaskListItems(fullFileContent: string): number {
  let count = 0;
  for (const _item of taskItemLines(fullFileContent)) count++;
  return count;
}

/** Toggle the status character of the Nth GFM task-list item (0-based,
 *  `taskIndex`) found in `fullFileContent`, top to bottom (see
 *  `taskItemLines`).
 *
 *  Splits and rejoins on `'\n'` only, never `/\r?\n/`, so every byte outside
 *  the one toggled status character is preserved exactly: line-ending
 *  style, a trailing (or missing) final newline, and a BOM all round-trip
 *  unchanged.
 *
 *  Returns `null` when `taskIndex` names no task item, e.g. the file changed
 *  concurrently in a way that removed it. The caller must not write in that
 *  case. */
export function toggleTaskListCheckbox(fullFileContent: string, taskIndex: number): string | null {
  const lines = fullFileContent.split('\n');
  let count = 0;

  for (const item of taskItemLines(fullFileContent)) {
    if (count === taskIndex) {
      const [prefix, statusChar] = [item.match[1], item.match[2]];
      const newChar = statusChar === ' ' ? 'x' : ' ';
      lines[item.index] = lines[item.index].slice(0, item.quoteLen) + prefix + newChar + ']' + item.rest.slice(item.match[0].length);
      return lines.join('\n');
    }
    count++;
  }

  return null;
}
