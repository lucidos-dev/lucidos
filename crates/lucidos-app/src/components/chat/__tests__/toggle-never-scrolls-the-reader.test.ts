import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, join, resolve } from 'node:path';

/** No collapse, expand or disclosure inside the transcript may move the reader.
 *
 *  This file was `toggle-marks-reader-parked.test.ts` and enforced the mirror
 *  image: every toggle had to call `preserveOnToggle()` BEFORE its growth
 *  landed, because the transcript's ResizeObserver would otherwise keep a
 *  bottom-riding reader on the bottom and scroll the thing they had just
 *  expanded off the top. Nothing re-pins on a resize any more, so the mark has
 *  no job and is gone; what remains is the other half of the same contract, and
 *  it is the half that can still be got wrong. A toggle is a disclosure, not a
 *  request to go to the live edge, so no toggle handler may scroll.
 *
 *  Why this stays a source scan and not a behavioural test: the failure is a
 *  NEW toggle site that reaches for `scrollToBottom()` to "make sure you see
 *  it", and no behavioural test can fail for a site it does not know exists.
 *  Three of the four sites forgot the old mark, for exactly this reason.
 *
 *  Two shapes of site, checked PER SITE rather than by a total, because a total
 *  cannot tell two scrolls in one handler apart from one in each of two:
 *   - an `onToggle` handler (a collapsible panel, a `<details>` disclosure), and
 *   - a `withScrollAnchor` call (the two turn controls), which is the
 *     legitimate way to hold the reader still across a turn's own growth.
 *
 *  Scanned across every component in `components/chat`, not just the one that
 *  folds a turn today: the queued-message group in `CreateThreadView.tsx` used
 *  to snap to the bottom when expanded, and a file-scoped scan would not have
 *  seen it.
 *
 *  A site that genuinely must scroll will fail here. Say why at the site and
 *  exclude it explicitly rather than weakening the scan. */
describe('no transcript toggle scrolls the reader', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const CHAT_DIR = resolve(here, '..');

  /** Anything that moves the transcript. `withScrollAnchor` is deliberately not
   *  here: it COMPENSATES for a toggle's growth to hold the reader on the same
   *  content, which is the opposite of moving them.
   *
   *  It is also the ONE place a reveal may legitimately move somebody, and the
   *  exception proves the rule rather than bending it. A reader riding the live
   *  edge asked to be kept there, and a transcript-wide reveal grows the turns
   *  below them, so `honourAnchoredMutation` puts them back on it (armed only,
   *  in one write, and never as something a toggle HANDLER decided). Everyone else is
   *  left exactly where the correction put them, which is what this scan is
   *  about: the failure it exists for is a new handler reaching for
   *  `scrollToBottom()` to "make sure you see it".
   *
   *  All four SUBMIT entry points are here, and they are worth listing in full
   *  even though each moves the reader LESS than a chevron does (a reader already
   *  at the live edge is not scrolled at all, and neither is one whose whole
   *  thread is on screen). A submit is the reader handing the agent something and
   *  being shown it picked up; a disclosure is the reader looking at what is
   *  already there. A toggle reaching for one would glide the transcript for an
   *  act nobody submitted. */
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

  /** Comments are prose ABOUT these calls, and this file's own history is full
   *  of prose naming them. Counting them would both invent sites and let a
   *  comment mentioning `scrollToBottom()` fail a clean handler.
   *
   *  LINE comments go first, unlike the block-then-line order in
   *  `utils/no-raw-storage.test.ts`. A `/*` sitting inside a line comment pairs
   *  with the next real block terminator several hundred lines below, so
   *  block-first swallowed whole functions and the scan silently found nothing
   *  to check. The `[^:]` guard keeps `https://` intact. */
  function stripComments(src: string): string {
    return src
      .replace(/(^|[^:])\/\/.*$/gm, '$1')
      .replace(/\/\*[\s\S]*?\*\//g, '');
  }

  /** The balanced-brace expression starting at `open` (the index of a `{`), or
   *  `null` if the braces never balance. `null`, never the tail: a tail runs to
   *  the end of the file and would sweep in some OTHER function's calls, so a
   *  mis-parse would read as a failure on the wrong site. A mis-parse must be
   *  reported as itself. */
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

  /** A pass-through, i.e. a handler slot filled with a bare identifier
   *  (`onToggle={onToggle}`) rather than a body. `chat-exchange-parts.tsx`
   *  forwards its panels' toggle prop that way in four places; a forward defines
   *  no behaviour, so counting it would inflate the site total and hide a real
   *  site being eaten by a bad strip. The behaviour lives where the arrow is
   *  written, which is what the counts below pin. */
  function isPassThrough(handler: string): boolean {
    return /^\{\s*[A-Za-z_$][\w$]*\s*\}$/.test(handler.trim());
  }

  /** True at the `withScrollAnchor` DEFINITION rather than a call of it. The
   *  helper lives in `CreateThreadView.tsx`, inside the same directory the scan
   *  sweeps, so its own signature matches the call pattern. */
  function isDefinition(src: string, at: number): boolean {
    return src.slice(Math.max(0, at - 20), at).includes('function ');
  }

  /** The sites that exist today. Pinned exactly, not as a `> 0` floor: a floor
   *  catches a strip that swallows a file WHOLE (which one did, so it earns its
   *  place) but not one that eats a single site, which is the same silent pass
   *  wearing a smaller hat. Adding or removing a toggle means changing these
   *  numbers deliberately, in the same commit. */
  const EXPECTED_PANEL_TOGGLES = 2;    // ChatExchange: the initiator panel, the response panel
  // ChatExchange's `reveal`, which the full-response and steps turn controls
  // both route through. It was one anchored site per control until they grew a
  // shared second half (turning either ON also lifts this turn's fold), which
  // has to happen inside the SAME anchor or the anchor measures a height the
  // unfold then changes.
  const EXPECTED_ANCHORED_TOGGLES = 1;

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

  it('no scroll-anchored toggle moves the transcript', () => {
    const offenders: string[] = [];
    let seen = 0;
    for (const { file, src } of sources) {
      const re = /withScrollAnchor\(/g;
      for (let m = re.exec(src); m !== null; m = re.exec(src)) {
        if (isDefinition(src, m.index)) continue;
        seen++;
        // Look back to the top of the enclosing handler. Both shapes count: a
        // `function` declaration and an arrow body, whichever starts closer, so
        // writing the next toggle as `const toggleX = () => {` cannot resolve
        // back past it into an earlier function.
        const start = Math.max(src.lastIndexOf('function ', m.index), src.lastIndexOf('=> {', m.index));
        const body = start < 0 ? '' : braced(src, src.indexOf('{', start)) ?? src.slice(start, m.index);
        const call = SCROLL_CALLS.find((c) => body.includes(c));
        if (call) offenders.push(`${file}: ${call} in ${body.replace(/\s+/g, ' ').slice(0, 100)}`);
      }
    }
    expect(seen, 'the scan has drifted off its target').toBe(EXPECTED_ANCHORED_TOGGLES);
    expect(
      offenders,
      'withScrollAnchor call(s) whose handler also moves the transcript. withScrollAnchor '
      + 'already holds the reader on their own topmost line across the growth; a scroll '
      + `beside it undoes exactly that:\n${offenders.join('\n')}`,
    ).toEqual([]);
  });
});
