import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

/** Every user-driven collapse/expand inside a turn must mark the reader parked
 *  (`preserveOnToggle()`) BEFORE the growth lands.
 *
 *  Why this is a source scan and not a behavioural test: the failure is a toggle
 *  site that simply forgets, and no behavioural test can fail for a site it does
 *  not know exists. Three of the four sites had forgotten, and it went unnoticed
 *  because the transcript's ResizeObserver used to infer the parked state from
 *  the growth itself. That inference is gone (it could not tell an expand apart
 *  from a markdown image decoding late, and stranded the reader when it guessed
 *  wrong), so the toggle now has to say so itself, and every site has to say it.
 *
 *  Two shapes of site, checked PER SITE rather than by a total, because a total
 *  cannot tell two marks in one handler apart from one in each of two:
 *   - an `onToggle` handler on a collapsible panel (the initiator panel, the
 *     response panel), and
 *   - a `withScrollAnchor` call (More/Less, Show steps/Hide steps), which holds
 *     the turn's ROOT still and therefore does nothing at all for growth that
 *     lands below it.
 *
 *  Scoped to `ChatExchange.tsx`, which is where a turn's own content is folded.
 *  The one `onToggle` outside it, the queued-message-group `<details>` in
 *  `CreateThreadView.tsx`, deliberately calls `scrollToBottom()` instead: it
 *  reveals queued messages the reader is being shown, not history they are
 *  reading.
 *
 *  A site that genuinely should not mark the reader will fail here. Say why at
 *  the site and exclude it explicitly rather than weakening the scan. */
describe('every ChatExchange toggle marks the reader parked', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const FILE = resolve(here, '../ChatExchange.tsx');

  /** Comments are prose ABOUT these calls, and this change added one naming
   *  `withScrollAnchor`. Counting them would both invent sites and let a comment
   *  mentioning `preserveOnToggle()` mask a real miss.
   *
   *  LINE comments go first, unlike the block-then-line order in
   *  `utils/no-raw-storage.test.ts`. This file contains `// … the /data/* static
   *  mount`, and a `/*` sitting inside a line comment pairs with the next real
   *  `*​/` several hundred lines below, so block-first swallowed both toggle
   *  functions and the scan silently found nothing to check. The `[^:]` guard
   *  keeps `https://` intact. */
  function stripComments(src: string): string {
    return src
      .replace(/(^|[^:])\/\/.*$/gm, '$1')
      .replace(/\/\*[\s\S]*?\*\//g, '');
  }

  /** The balanced-brace expression starting at `open` (the index of a `{`), or
   *  `null` if the braces never balance. `null`, never the tail: a tail runs to
   *  the end of the file and would contain some OTHER site's mark, so a
   *  mis-parse would read as a pass. A mis-parse must be reported. */
  function braced(src: string, open: number): string | null {
    let depth = 0;
    for (let i = open; i < src.length; i++) {
      if (src[i] === '{') depth++;
      else if (src[i] === '}' && --depth === 0) return src.slice(open, i + 1);
    }
    return null;
  }

  const src = stripComments(readFileSync(FILE, 'utf-8') as string);

  /** The sites that exist today. Pinned exactly, not as a `> 0` floor: a floor
   *  catches a strip that swallows the file WHOLE (which one did, so it earns
   *  its place) but not one that eats a single site, which is the same silent
   *  pass wearing a smaller hat. Adding or removing a toggle means changing
   *  these two numbers deliberately, in the same commit. */
  const EXPECTED_PANEL_TOGGLES = 2;   // the initiator panel, the response panel
  const EXPECTED_ANCHORED_TOGGLES = 2; // More/Less, Show steps/Hide steps

  it('marks the reader parked in every panel onToggle handler', () => {
    const offenders: string[] = [];
    let seen = 0;
    const re = /onToggle=\{/g;
    for (let m = re.exec(src); m !== null; m = re.exec(src)) {
      seen++;
      const handler = braced(src, m.index + 'onToggle='.length);
      if (handler === null) {
        offenders.push(`<unparseable handler at offset ${m.index}>`);
      } else if (!handler.includes('preserveOnToggle()')) {
        offenders.push(handler.replace(/\s+/g, ' ').slice(0, 120));
      }
    }
    expect(seen, 'the scan has drifted off its target').toBe(EXPECTED_PANEL_TOGGLES);
    expect(
      offenders,
      'onToggle handler(s) that grow the turn without marking the reader parked, so the '
      + "transcript's resize handler keeps a bottom-riding reader on the bottom and scrolls "
      + `the thing they just expanded off the top:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });

  it('marks the reader parked before every scroll-anchored toggle', () => {
    const offenders: string[] = [];
    let seen = 0;
    const re = /withScrollAnchor\(/g;
    for (let m = re.exec(src); m !== null; m = re.exec(src)) {
      seen++;
      // The mark has to precede the anchored mutation, so look back to the top
      // of the enclosing handler. Both shapes count: a `function` declaration
      // and an arrow body, whichever starts closer, so writing the next toggle
      // as `const toggleX = () => {` cannot resolve back past it to an earlier
      // function whose mark would then satisfy this.
      const start = Math.max(src.lastIndexOf('function ', m.index), src.lastIndexOf('=> {', m.index));
      const before = start < 0 ? '' : src.slice(start, m.index);
      if (!before.includes('preserveOnToggle()')) {
        offenders.push(before.replace(/\s+/g, ' ').slice(0, 120) || '<no enclosing handler>');
      }
    }
    expect(seen, 'the scan has drifted off its target').toBe(EXPECTED_ANCHORED_TOGGLES);
    expect(
      offenders,
      'withScrollAnchor call(s) not preceded by preserveOnToggle() in the same function. '
      + 'withScrollAnchor only holds the turn ROOT still, so it does nothing for growth below '
      + `it and cannot stand in for the mark:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });
});
