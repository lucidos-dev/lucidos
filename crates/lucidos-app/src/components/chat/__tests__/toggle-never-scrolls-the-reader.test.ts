import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join, resolve } from 'node:path';

describe('no transcript toggle scrolls the reader', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const CHAT_DIR = resolve(here, '..');

  const SCROLL_CALLS = [
    'scrollToBottom',
    'scrollToBottomAnimated',
    'scrollToTop',
    'followSentMessage',
    'followAnsweredQuestion',
    'followResolvedPermission',
    'followContinuedThread',
    'pinToBottomNow',
    'scrollIntoView',
    'scrollTop =',
  ];

  function stripComments(src: string): string {
    return src
      .replace(/(^|[^:])\/\/.*$/gm, '$1')
      .replace(/\/\*[\s\S]*?\*\//g, '');
  }

  function braced(src: string, open: number): string | null {
    let depth = 0;
    for (let i = open; i < src.length; i++) {
      if (src[i] === '{') depth++;
      else if (src[i] === '}' && --depth === 0) return src.slice(open, i + 1);
    }
    return null;
  }

  const sources = (readdirSync(CHAT_DIR) as string[])
    .filter((f) => f.endsWith('.tsx'))
    .map((f) => ({ file: f, src: stripComments(readFileSync(join(CHAT_DIR, f), 'utf-8') as string) }));

  function isPassThrough(handler: string): boolean {
    return /^\{\s*[A-Za-z_$][\w$]*\s*\}$/.test(handler.trim());
  }

  function isDefinition(src: string, at: number): boolean {
    return src.slice(Math.max(0, at - 20), at).includes('function ');
  }

  const EXPECTED_PANEL_TOGGLES = 1;
  const EXPECTED_ANCHORED_TOGGLES = 4;

  it('no onToggle handler moves the transcript', () => {
    const offenders: string[] = [];
    let seen = 0;
    for (const { file, src } of sources) {
      const re = /onToggle=\{/g;
      for (let m = re.exec(src); m !== null; m = re.exec(src)) {
        seen++;
        const handler = braced(src, m.index + 'onToggle='.length);
        if (handler === null) {
          offenders.push(`${file}: <unparseable handler at offset ${m.index}>`);
          continue;
        }
        if (isPassThrough(handler)) { seen--; continue; }
        const call = SCROLL_CALLS.find((c) => handler.includes(c));
        if (call) offenders.push(`${file}: ${call} in ${handler.replace(/\s+/g, ' ').slice(0, 100)}`);
      }
    }
    expect(seen, 'the scan has drifted off its target').toBe(EXPECTED_PANEL_TOGGLES);
    expect(
      offenders,
      'onToggle handler(s) that move the transcript. A disclosure is not the "take me to the '
      + 'live edge" gesture: growing content under the reader must leave them where they are, '
      + `and the chevron is how they follow it:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });

  function parenthesised(src: string, open: number): string | null {
    let depth = 0;
    for (let i = open; i < src.length; i++) {
      if (src[i] === '(') depth++;
      else if (src[i] === ')' && --depth === 0) return src.slice(open, i + 1);
    }
    return null;
  }

  function pressedWork(src: string, arg: string): string {
    let body = arg;
    for (const [, name] of arg.matchAll(/([A-Za-z_$][\w$]*)\s*\(/g)) {
      const at = src.search(new RegExp(`\\bfunction ${name}\\s*\\(`));
      if (at < 0) continue;
      body += '\n' + (braced(src, src.indexOf('{', at)) ?? '');
    }
    return body;
  }

  it('no scroll-anchored toggle moves the transcript', () => {
    const offenders: string[] = [];
    let seen = 0;
    for (const { file, src } of sources) {
      const re = /heldOnThePress\(/g;
      for (let m = re.exec(src); m !== null; m = re.exec(src)) {
        if (isDefinition(src, m.index)) continue;
        seen++;
        const arg = parenthesised(src, m.index + 'heldOnThePress'.length);
        if (arg === null) {
          offenders.push(`${file}: <unparseable press at offset ${m.index}>`);
          continue;
        }
        const body = pressedWork(src, arg);
        const call = SCROLL_CALLS.find((c) => body.includes(c));
        if (call) offenders.push(`${file}: ${call} in ${body.replace(/\s+/g, ' ').slice(0, 100)}`);
      }
    }
    expect(seen, 'the scan has drifted off its target').toBe(EXPECTED_ANCHORED_TOGGLES);
    expect(
      offenders,
      'withScrollAnchor call(s) whose handler also moves the transcript. withScrollAnchor '
      + 'already holds the control the reader pressed across the growth; a scroll '
      + `beside it undoes exactly that:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });
});
