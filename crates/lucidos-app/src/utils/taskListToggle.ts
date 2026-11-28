import { frontmatterLineCount } from './frontmatter';

const TASK_ITEM = /^(\s*(?:[-*+]|\d{1,9}[.)])\s+\[)([ xX])\](?=[ \t]|\r?$)/;

const FENCE_LINE = /^ {0,3}(`{3,}|~{3,})(.*)$/;

const BLOCKQUOTE_PREFIX = /^(?: {0,3}>[ \t]?)+/;

interface FenceMatch { char: string; len: number; rest: string }

function matchFence(line: string): FenceMatch | null {
  const m = FENCE_LINE.exec(line);
  if (!m) return null;
  return { char: m[1][0], len: m[1].length, rest: m[2].trim() };
}

interface TaskItemLine {
  index: number;
  quoteLen: number;
  rest: string;
  match: RegExpExecArray;
}

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
      continue;
    }

    const opening = matchFence(rest);
    if (opening) {
      fence = { char: opening.char, len: opening.len };
      continue;
    }

    const match = TASK_ITEM.exec(rest);
    if (!match) continue;
    yield { index: i, quoteLen, rest, match };
  }
}

export function countTaskListItems(fullFileContent: string): number {
  let count = 0;
  for (const _item of taskItemLines(fullFileContent)) count++;
  return count;
}

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
