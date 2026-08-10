/**
 * A turn header's status + timestamp stay a right-hand column when the row
 * runs out of width.
 *
 * Both turn headers put the status and the timestamp in one right-aligned
 * cluster: `.response-meta` (chat/response.css) and `.initiator-meta`
 * (chat/input-messages.css). As a rigid content-width item that cluster was
 * bumped whole onto a second row the moment it stopped fitting, leaving the
 * executor chip and the turn controls alone on a row with a hand of empty
 * space to their right and "Working Today 12:00:52" floated underneath. What
 * the user asked for is the pair stacking in place: status on the executor's
 * row, timestamp under it, both flush right.
 *
 * Five declarations produce that, and dropping any ONE of them silently gives
 * back a different wrong layout, which is why they are pinned as a set rather
 * than one by one:
 *
 *   - `flex: 1 1 0` makes the cluster elastic, so it takes the row's leftover
 *     width instead of sitting at content width and being wrapped away whole.
 *     A `flex-shrink: 0` (what it replaced) or a bare `flex-grow` on a content
 *     basis puts the old behaviour straight back.
 *   - `flex-wrap: wrap` is what lets it break between its own two fields.
 *   - `justify-content: flex-end` is the right alignment, in BOTH states. With
 *     the cluster elastic there is no free space left for the `margin-left:
 *     auto` that used to do this, so without it every line goes flush LEFT,
 *     which is the specific regression the user called out.
 *   - `white-space: nowrap` picks where the break lands. Without it the box
 *     shrinks by breaking the timestamp's own space first, giving "Today" over
 *     "12:00:52" with the status still beside them: a break inside a field
 *     instead of between the two.
 *   - `row-gap: 0` keeps the stacked pair reading as one cluster. The old
 *     `gap: 0.5rem` shorthand set a row gap too, which never applied while the
 *     cluster could not wrap and opens 8px between the two lines now that it
 *     can.
 *
 * The two rules are deliberate copies, one per component file, and the headers
 * sit one above the other inside a turn: if they degrade differently they read
 * as two different widgets. Nothing but this test holds the copies together.
 *
 * The other half of the contract is WHERE the row's other fields sit once the
 * cluster is two lines, and it is pinned here because it is the same layout
 * seen from the other side. The header aligns its fields at `flex-start` and
 * every field is one `--turn-header-line` tall, centring its own content:
 *
 *   - `flex-start` alone is wrong. The fields are not naturally the same
 *     height, so it un-centres the turn controls against the executor's name on
 *     every ordinary one-row header, which is all but one row in a transcript.
 *   - `center` alone is wrong too, and is what shipped: once the cluster is two
 *     lines it is the tallest thing on the row, so the name centres against
 *     BOTH of its lines and lands half a line below the status beside it. That
 *     is the user report this replaced ("Working and Claude Code must have same
 *     top alignment").
 *   - Together they are neither. Every field resolves to the same box, so a
 *     one-row header is pixel for pixel what centring gave, and a wrapped one
 *     puts the first field's text on the name's line by construction.
 *
 * So the row unit has to reach every field, and each one earns its own
 * assertion below: the chip (whose negative margins mean the line measures a
 * box two padding steps smaller than its own), the turn controls, and the
 * cluster's FIRST field, which is the status when there is one and the
 * timestamp on a turn with none. Drop any of them and that field alone drifts
 * off the line, which is subtle enough to survive review.
 *
 * A source scan because nothing else can see it. `tsc` does not read CSS,
 * `vite build` only parses it, and jsdom runs no layout, so a Vitest render
 * cannot answer where a flex line breaks. Parsed with postcss rather than
 * matched textually, so a later rule re-rigidifying either cluster is caught.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, rulesTargeting, type CssRule } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf8');

/** The two sheets under scan. Named because the per-field assertions reach for
 *  one of them directly (the chip rule is shared and lives with the initiator,
 *  the turn controls only exist on a response). */
const RESPONSE_CSS = read('../chat/response.css');
const INITIATOR_CSS = read('../chat/input-messages.css');

/** The two header clusters, each with the sheet its rules live in and the row
 *  they sit on. */
const CLUSTERS = [
  { cls: 'response-meta', header: 'response-header', css: RESPONSE_CSS },
  { cls: 'initiator-meta', header: 'initiator-header', css: INITIATOR_CSS },
];

/** What the stack-right-aligned behaviour is made of, property by property. */
const CONTRACT: [string, string][] = [
  ['flex', '1 1 0'],
  ['flex-wrap', 'wrap'],
  ['justify-content', 'flex-end'],
  ['white-space', 'nowrap'],
  ['row-gap', '0'],
  ['column-gap', '0.5rem'],
];

/** Every rule that styles the cluster element itself, in source order. */
const rulesFor = (cls: string, css: string): CssRule[] => rulesTargeting(css, cls);

/** The value the cascade lands on for `prop`, last declaration winning. */
function effective(rules: CssRule[], prop: string): string | undefined {
  let value: string | undefined;
  for (const rule of rules) {
    const own = rule.props.get(prop);
    if (own !== undefined) value = own;
  }
  return value;
}

/** The rule sizing a cluster's first field, which no `rulesTargeting` call can
 *  find: the subject of `.response-meta > :first-child` is the child, not the
 *  cluster. */
function firstFieldRule(cls: string, css: string): CssRule | undefined {
  return cssRules(css).find(rule => rule.selector === `.${cls} > :first-child`);
}

describe('turn header meta stacking', () => {
  for (const { header, css } of CLUSTERS) {
    it(`.${header} aligns at flex-start on the shared row unit`, () => {
      // Half of the pair described in the file header. On its own this reads
      // like the mistake the previous version of this test guarded against, so
      // the row-unit assertions below are what make it correct: they are the
      // reason flex-start cannot un-centre anything, and deleting one of them
      // silently turns this line back into that mistake.
      const rules = rulesFor(header, css);
      expect(effective(rules, 'align-items'), `.${header} must align its fields at the top`)
        .toBe('flex-start');
      // The row unit is also the row's floor, which is the job this min-height
      // already had as a bare 1.2rem. It has to be the SAME value the fields
      // are sized to, or a one-row header stretches its line past them and
      // flex-start stops agreeing with centring.
      expect(effective(rules, 'min-height'), `.${header} must floor at the row unit`)
        .toBe('var(--turn-header-line)');
    });
  }

  it('sizes the actor/executor chip to the row unit, padding added back', () => {
    // The chip cancels its own vertical padding with a negative margin so the
    // hover surface reaches past the text for free. A flex line measures margin
    // boxes, so a bare `min-height: var(--turn-header-line)` here resolves to a
    // margin box two padding steps SHORT of the unit, and the name sits that
    // much above the status. Both halves are read off one local var so they
    // cannot drift apart.
    const chip = cssRules(INITIATOR_CSS).find(r => r.selector === '.initiator-actor, .response-executor');
    expect(chip, 'the shared actor/executor chip rule must exist').toBeDefined();
    expect(chip?.props.get('--actor-chip-pad-y')).toBe('0.125rem');
    expect(chip?.props.get('padding')).toBe('var(--actor-chip-pad-y) 0.375rem');
    expect(chip?.props.get('margin')).toBe('calc(-1 * var(--actor-chip-pad-y)) -0.375rem');
    expect(chip?.props.get('min-height'))
      .toBe('calc(var(--turn-header-line) + 2 * var(--actor-chip-pad-y))');
  });

  it('sizes the turn controls to the row unit', () => {
    // The only field with no margin trick, so it takes the unit as written.
    expect(effective(rulesTargeting(RESPONSE_CSS, 'response-controls'), 'min-height'))
      .toBe('var(--turn-header-line)');
  });

  for (const { cls, css } of CLUSTERS) {
    it(`sizes .${cls}'s first field to the row unit and centres its text`, () => {
      // The field that shares the name's line: the status when there is one,
      // the timestamp on a chromeless turn. `min-height` puts its box on the
      // unit and `align-items: center` puts its text in the middle of that box,
      // which is what lands it on the name's line rather than at the top of it.
      // Only the FIRST field: sizing the second would make the stacked
      // timestamp a full unit tall and open the gap `row-gap: 0` closes.
      const rule = firstFieldRule(cls, css);
      expect(rule, `.${cls} > :first-child must be sized`).toBeDefined();
      expect(rule?.props.get('min-height')).toBe('var(--turn-header-line)');
      expect(rule?.props.get('display')).toBe('flex');
      expect(rule?.props.get('align-items')).toBe('center');
    });
  }

  for (const { cls, css } of CLUSTERS) {
    describe(`.${cls}`, () => {
      const rules = rulesFor(cls, css);

      it('is styled by at least one rule, so the scan is looking at something', () => {
        expect(rules.length, `no rule targets .${cls}`).toBeGreaterThan(0);
      });

      for (const [prop, want] of CONTRACT) {
        it(`sets ${prop}: ${want}`, () => {
          expect(effective(rules, prop), `.${cls} must end up at ${prop}: ${want}`).toBe(want);
        });
      }

      it('never goes rigid again, whatever a later rule says', () => {
        // `flex-shrink: 0` was the old declaration and is the exact thing that
        // wrapped the cluster away whole; a lone `flex-basis` on the content
        // size does the same without naming the old property.
        expect(effective(rules, 'flex-shrink'), `.${cls} must stay shrinkable`).toBeUndefined();
        expect(effective(rules, 'flex-basis'), `.${cls} must keep the zero basis from its shorthand`)
          .toBeUndefined();
      });

      it('does not lean on the auto margin the elastic basis made inert', () => {
        // `margin-left: auto` only right-aligns while there is free space on
        // the line, and there is none once the cluster grows into it. Leaving
        // it in reads as the thing doing the alignment, which sends the next
        // reader to delete `justify-content` instead.
        expect(effective(rules, 'margin-left'), `.${cls} right-aligns via justify-content now`)
          .not.toBe('auto');
      });
    });
  }

  it('defines the row unit the fields are sized to', () => {
    // Every assertion above names `var(--turn-header-line)`, and an undefined
    // custom property in a `min-height` is invalid at computed-value time: the
    // declaration is dropped, every field falls back to its natural height, and
    // the alignment quietly goes back to what the user reported. A typo'd or
    // deleted token therefore has to fail here rather than on screen.
    const base = read('../global/base.css');
    expect(base, 'base.css :root must define --turn-header-line')
      .toMatch(/--turn-header-line:\s*[^;]+;/);
  });

  it('gives the two headers the same contract, so a turn degrades as one', () => {
    const [response, initiator] = CLUSTERS.map(({ cls, css }) => {
      const rules = rulesFor(cls, css);
      const field = firstFieldRule(cls, css);
      return [
        ...CONTRACT.map(([prop]) => `${prop}: ${effective(rules, prop)}`),
        // The first field's sizing joins the comparison for the same reason the
        // wrap contract is in it: a turn's two headers are read as one widget,
        // so one of them aligning its status differently is the same class of
        // bug as one of them wrapping differently.
        `first-field: ${field?.props.get('display')} ${field?.props.get('align-items')} ${field?.props.get('min-height')}`,
      ].join('; ');
    });
    expect(initiator, '.initiator-meta and .response-meta must wrap identically').toBe(response);
  });
});
