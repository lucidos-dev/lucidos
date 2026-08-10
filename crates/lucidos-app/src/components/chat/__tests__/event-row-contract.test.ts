import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

/** Comments are stripped before every scan below, because each of these files
 *  explains at length what it used to be (`.inline-step`, `.event-delivery-name`)
 *  and why it stopped. That history is exactly the context the repo wants kept,
 *  so a scan that reads it as a live reference would punish the documentation it
 *  depends on. `//` is matched only at the start of a line so a URL survives. */
const code = (src: string): string =>
  src
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .split('\n')
    .filter((l) => !/^\s*(\/\/|\*)/.test(l))
    .join('\n');

/** **The event row is not a step, and these are the properties that keep it
 *  that way.**
 *
 *  The row rendered through `.inline-step` until 2026-08-10, which had two
 *  consequences the render tests next door cannot see, because both live in CSS
 *  or in an import: a green success check on a live subscription, and a
 *  single-line ellipsis over the reason and the subscription, which are the
 *  only two things the row has to say.
 *
 *  Source scans rather than a browser test, deliberately. There is no jsdom in
 *  this test infra, and the failure mode is somebody reaching for the step
 *  list's classes again because they are right there and look close enough. See
 *  `docs/plans/2026-08-10-one-event-row-for-the-transcript.md`. */
describe('event row contract', () => {
  const css = read('../../../styles/chat/event-rows.css');
  const row = read('../EventRow.tsx');
  const parts = read('../chat-exchange-parts.tsx');
  const child = read('../ChildCompletionRow.tsx');

  function block(selector: string): string {
    const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const re = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g');
    return [...css.matchAll(re)].map((m) => m[1]).join('\n');
  }

  /** **The truncation half of the reported bug.** The subject is a sentence
   *  somebody wrote, and it is the reason the row exists: an ellipsis there
   *  hides the answer to "waiting for what". */
  it('never truncates the subject', () => {
    const subject = block('.event-row-subject');
    expect(subject).toContain('overflow-wrap');
    expect(subject).not.toContain('white-space: nowrap');
    expect(subject).not.toContain('text-overflow');
  });

  /** The facts line wraps as a whole instead, so a narrow thread pane stacks
   *  the chips rather than clipping them off the right edge. */
  it('wraps the facts line rather than clipping it', () => {
    const meta = block('.event-row-meta');
    expect(meta).toContain('flex-wrap: wrap');
    expect(meta).not.toContain('text-overflow');
  });

  /** **The green-check half.** The mark reports that something happened, never
   *  that it succeeded. The moment it can carry a verdict colour it is a step
   *  icon again, so the only colour it may name is the muted one. */
  it('keeps the mark column muted', () => {
    const mark = block('.event-row-mark');
    expect(mark).toContain('color: var(--text-muted)');
    for (const verdict of ['--accent-green', '--accent-red', '--accent-yellow']) {
      expect(mark).not.toContain(verdict);
    }
  });

  /** No module in the family may reach for the step list's outcome helper or
   *  its classes. `stepStatus` is what mapped `waiting` to `success`. */
  it.each([
    ['EventRow.tsx', () => row],
    ['chat-exchange-parts.tsx event-wait row', () => parts.slice(parts.indexOf('export function eventWaitRowBody'))],
    ['ChildCompletionRow.tsx', () => child],
  ])('%s takes no step outcome', (_name, source) => {
    const src = code(source());
    expect(src).not.toContain('stepStatus');
    expect(src).not.toContain('inline-step');
    expect(src).not.toContain('step-icon');
  });

  /** One atom for an event type, so a subscription, a delivery and an
   *  event-fired trigger spell the same word the same way. A second
   *  accent-tinted mono chip rule is the drift this guards. */
  it('defines exactly one event-name chip', () => {
    expect([...css.matchAll(/^\.event-name\s*\{/gm)]).toHaveLength(1);
    expect(code(css)).not.toContain('.event-delivery-name');
  });

  /** Every tint is a `color-mix` over a token, so both themes resolve from one
   *  rule and no state pill hardcodes a hex. */
  it('tints every state from a token', () => {
    const tones = [...css.matchAll(/\.event-row-state\[data-tone="[a-z]+"\]\s*\{([^}]*)\}/g)];
    expect(tones.length).toBeGreaterThanOrEqual(6);
    for (const [, body] of tones) {
      expect(body).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
      expect(body).toMatch(/var\(--/);
    }
  });

  /** The banned template look, checked here because this file introduces the
   *  transcript's newest surface and is exactly where one would creep back in
   *  (`.claude/rules/frontend-css.md`). A `border-left` shorthand cannot appear
   *  even as part of the card's own all-round border, so the scan is for the
   *  longhand specifically. */
  it('adds no left accent stripe', () => {
    expect(css).not.toMatch(/border-left\s*:/);
    expect(css).not.toMatch(/box-shadow:\s*inset/);
  });

  /** **The card is lighter than the affordance card it sits beside.** The boxed
   *  `.step-note-card` weight is what an inline affordance earns (the
   *  checkpoint's Undo), so an event row lifts on `--bg-secondary` with a
   *  hairline instead of taking the tertiary fill and the full border. Losing
   *  that gap would make a record look like something you can act on. */
  it('is a card, and a lighter one than .step-note-card', () => {
    const row = block('.event-row');
    expect(row).toContain('background: var(--bg-secondary)');
    expect(row).toContain('border-radius');
    expect(row).not.toContain('background: var(--bg-tertiary)');
  });

  /** The subject and the state share the top line, which is what stops the card
   *  spending a whole line on one word. The pill pins to the FIRST line, so a
   *  subject wrapping to three lines still reports its verdict where the reading
   *  starts rather than trailing the last line. */
  it('puts the state on the subject line, pinned to the top', () => {
    expect(block('.event-row-head')).toContain('display: flex');
    const state = block('.event-row-state');
    expect(state).toContain('align-self: flex-start');
    expect(state).toContain('flex: 0 0 auto');
    // The subject is the only child that may give, so a long one wraps instead
    // of pushing the pill off the card's right edge.
    expect(block('.event-row-subject')).toContain('min-width: 0');
  });

  /** A fold on the card's own fill would open onto an invisible panel. */
  it('lifts the fold body off the card fill', () => {
    const fold = block('.event-row-fold > pre,\n.event-row-fold > .event-row-fold-body');
    expect(fold).toContain('background: var(--bg-tertiary)');
  });
});
