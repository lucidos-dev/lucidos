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

  /** **No leading glyph.** The subject and the state word say what happened,
   *  and a glyph carrying a verdict colour would be a step icon. */
  it('draws no mark column', () => {
    expect(code(row)).not.toContain('event-row-mark');
    expect(code(css)).not.toContain('event-row-mark');
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
   *  rule and no state word hardcodes a hex. */
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
   *  spending a whole line on one word. Flex baseline alignment uses the FIRST
   *  baseline. So a subject wrapping to three lines still reports its verdict
   *  level with the line where the reading starts. */
  it('puts the state on the subject line, on its first baseline', () => {
    const head = block('.event-row-head');
    expect(head).toContain('display: flex');
    expect(head).toContain('align-items: baseline');
    const state = block('.event-row-state');
    expect(state).not.toContain('align-self');
    expect(state).toContain('flex: 0 0 auto');
    // The subject is the only child that may give, so a long one wraps instead
    // of pushing the state off the card's right edge.
    expect(block('.event-row-subject')).toContain('min-width: 0');
  });

  /** The state is tinted text, never a pill. A lowercase word in a filled
   *  capsule read as a code token rather than a status. */
  it('draws the state as plain text, with no pill behind it', () => {
    const rules = [...css.matchAll(/\.event-row-state(\[data-tone="[a-z]+"\])?\s*\{([^}]*)\}/g)];
    expect(rules.length).toBeGreaterThanOrEqual(7);
    for (const [, , body] of rules) {
      expect(body).not.toMatch(/background|border-radius|padding/);
    }
  });

  /** **The phone half.** The subject may break anywhere, so its min-content is
   *  one letter. With the state fixed beside it, a phone-width pane squeezed it
   *  to one letter per line. The head wraps instead, and the subject keeps a
   *  basis, so the state drops to a line of its own at the far edge. */
  it('wraps the state below a subject that has no room', () => {
    const head = block('.event-row-head');
    expect(head).toContain('flex-wrap: wrap');
    expect(block('.event-row-subject')).toMatch(/flex:\s*1 1 \d+(\.\d+)?rem/);
    expect(block('.event-row-state')).toContain('margin-left: auto');
  });

  /** The time is a line of its own ABOVE the card, right-aligned, where a user
   *  bubble carries its stamp. */
  it('draws the time above the card, right-aligned', () => {
    const time = block('.event-row-time');
    expect(time).toContain('display: block');
    expect(time).toContain('text-align: right');
    expect(code(row)).toMatch(/event-row-time[\s\S]*\{card\}/);
  });

  /** A fold on the card's own fill would open onto an invisible panel. */
  it('lifts the fold body off the card fill', () => {
    const fold = block('.event-row-fold-pre,\n.event-row-fold-body');
    expect(fold).toContain('background: var(--bg-tertiary)');
  });
});
