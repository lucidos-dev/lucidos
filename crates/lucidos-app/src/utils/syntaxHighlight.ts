import { signal, type ReadonlySignal } from '@preact/signals';
import type { HLJSApi } from 'highlight.js';
import { escapeHtml } from './escapeHtml';
import { lazyModule } from './lazyComponent';

const highlighter = signal<HLJSApi | null>(null);
export const syntaxHighlighter: ReadonlySignal<HLJSApi | null> = highlighter;
const library = lazyModule(() => import('./highlightLibrary'));
let requested = false;

export async function loadSyntaxHighlighter(): Promise<void> {
  requested = true;
  if (await library.preload()) highlighter.value = library.get().default;
}

const EXT_TO_LANG: Record<string, string> = {
  js: 'javascript', jsx: 'javascript', gs: 'javascript',
  ts: 'typescript', tsx: 'typescript',
  py: 'python', rb: 'ruby', go: 'go', rs: 'rust',
  java: 'java', kt: 'kotlin', kts: 'kotlin',
  c: 'c', cpp: 'cpp', h: 'cpp',
  sh: 'bash', bash: 'bash', zsh: 'bash',
  sql: 'sql', css: 'css',
  yaml: 'yaml', yml: 'yaml', toml: 'ini',
  json: 'json', xml: 'xml', html: 'xml', htm: 'xml',
  graphql: 'javascript',
  vue: 'xml', svelte: 'xml',
};

export const CODE_EXTS = Object.keys(EXT_TO_LANG);

export function highlightFileLines(code: string, ext: string): string[] {
  const lang = EXT_TO_LANG[ext];
  if (!lang) return code.split('\n').map(escapeHtml);
  const hljs = highlighter.value;
  if (!hljs) {
    if (!requested) void loadSyntaxHighlighter();
    return code.split('\n').map(escapeHtml);
  }

  let html: string;
  try {
    html = hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
  } catch {
    return code.split('\n').map(escapeHtml);
  }

  return splitHighlightedLines(html);
}

function splitHighlightedLines(html: string): string[] {
  const rawLines = html.split('\n');
  const result: string[] = [];
  let openSpans: string[] = [];

  for (const line of rawLines) {
    const prefix = openSpans.join('');

    const newStack = [...openSpans];
    const tagRegex = /<(\/?)span([^>]*)>/g;
    let m;
    while ((m = tagRegex.exec(line)) !== null) {
      if (m[1] === '/') {
        if (newStack.length > 0) newStack.pop();
      } else {
        newStack.push(`<span${m[2]}>`);
      }
    }

    const suffix = '</span>'.repeat(newStack.length);
    result.push(prefix + line + suffix);
    openSpans = newStack;
  }

  return result;
}
