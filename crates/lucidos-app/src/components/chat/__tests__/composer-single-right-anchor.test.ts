import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

// The composer's right edge carries exactly ONE control, the send/stop morph.
//
// It used to carry two. A clear-draft button sat in the top-right corner of
// `.prompt-row` while the send sat in `.prompt-actions-row` below, which made a
// two-corner composition out of a frame with one row of controls. Measured at
// the desktop root, the two circles' centres were 6px apart (each inset by a
// different rule, at a different diameter), their vertical distance was
// whatever the textarea happened to be tall, and the corner glyph drew at 14px
// in --text-muted where every other icon in the composer is 20px in
// --text-secondary. The mobile override then made that same glyph 22.5px, which
// is LARGER than the send, so the pair's size relationship inverted between
// viewports.
//
// It also cost the field its right content edge: an in-flow flex sibling keeps
// its width, margin and the row gap even at `visibility: hidden`, so the typed
// text stopped 51px short of the box on the right against 13px on the left, in
// every state including the empty resting one.
//
// A source scan rather than a render test because every property here is
// cascade- and layout-resolved, which jsdom does not do. The rendered halves
// are covered by e2e/prompt-transcript-alignment.spec.ts (the field's two
// content insets now match, measured geometrically on both sides) and
// e2e/drafts.spec.ts (the button still clears).
const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');
const composerCss = readFileSync(
  resolve(here, '../../../styles/chat/input-messages.css'),
  'utf-8',
);
const mobileCss = readFileSync(resolve(here, '../../../styles/mobile.css'), 'utf-8');
/** The shared renderer, which is where the button's classes come from now. */
const headerActionsSource = readFileSync(
  resolve(here, '../../layout/headerActions.tsx'),
  'utf-8',
);

/** Index of `needle`, or a throw naming what the file was expected to contain. */
function at(needle: string, from = 0): number {
  const i = promptSource.indexOf(needle, from);
  if (i < 0) throw new Error(`${needle} not found in PromptInput.tsx`);
  return i;
}

/** Everything between the text row's opening tag and the prompt row's.
 *
 *  Deliberately NOT `.prompt-row`'s own `</div>`: `indexOf('</div>')` would stop
 *  at the first NESTED close if anything is ever wrapped inside the row, and a
 *  control added after that wrapper would fall outside the window this guard
 *  searches. Running to the next row's opening tag cannot shrink that way. The
 *  span picks up the hidden file `<input>` that sits between the two rows, which
 *  is not a `<button>` and so does not weaken the assertion. */
function textRowSpan(): string {
  const open = at('<div class="prompt-row">');
  return promptSource.slice(open, at('<div class={rowClass}', open));
}

/** The clear-draft ACTION's own source.
 *
 *  It is a `HeaderActionSpec` now, not JSX: the row's middle folds into a ⋯
 *  menu, and a spec is what renders either as the icon or as a menu row. So the
 *  span runs from the push that adds it to that push's close.
 *
 *  Anchored on the key, and closed on a four-space `});`. A bare `'});'` search
 *  would stop inside the handler, at `updateCompose(id, { text: '' });`, and
 *  every assertion below would then read half a spec. */
function clearSpec(): string {
  const span = /foldActions\.push\(\{\n\s+key: 'prompt-clear'[\s\S]*?\n {4}\}\);/.exec(promptSource);
  if (!span) throw new Error('no prompt-clear action is pushed onto foldActions in PromptInput.tsx');
  return span[0];
}

/** Every `.prompt-clear` rule body in a stylesheet, media queries included.
 *
 *  Comments are stripped first, or a mention of the class in prose (this file's
 *  own explanation of why there is no rule, for one) would be read as a selector
 *  and the scan would run past the comment to the NEXT rule's body, reporting
 *  that innocent rule's declarations as the violation. */
function clearRuleBodies(css: string): string[] {
  const bodies: string[] = [];
  const selector = /(^|[\s,{}])\.prompt-clear\b[^{}]*\{([^}]*)\}/g;
  let match: RegExpExecArray | null;
  const source = css.replace(/\/\*[\s\S]*?\*\//g, '');
  while ((match = selector.exec(source)) !== null) bodies.push(match[2]);
  return bodies;
}

describe('the composer has one control on its right edge', () => {
  it('renders no button in the text row', () => {
    expect(textRowSpan()).not.toMatch(/<button\b/);
  });

  // The fold cluster sits between the row's pinned leading controls and its
  // right-hand group. So the clear action lands in the left cluster wherever
  // the fold puts it.
  it('renders the fold cluster inside .prompt-actions-row', () => {
    expect(at('<OverflowMenu')).toBeGreaterThan(at('<div class={rowClass}'));
  });

  it('places the fold cluster ahead of the right-hand action group', () => {
    // The right group is `margin-left: auto`, and it keeps a single anchor.
    expect(at('<OverflowMenu')).toBeLessThan(at('<div class="prompt-actions-right">'));
  });

  it('keeps the clear action last of the row\'s MIDDLE', () => {
    // The fold takes a PREFIX, so a later member folds later. What may follow
    // clear is the right-hand cluster and the two fixed toggles, both of which
    // fold after it by design.
    const spec = clearSpec();
    const after = promptSource.slice(promptSource.indexOf(spec) + spec.length);
    expect(after).toMatch(/const middleActions = foldActions\.slice\(\);/);
    expect(after.slice(0, after.indexOf('const middleActions')))
      .not.toMatch(/foldActions\.push\(/);
  });
});

describe('the clear button is one of the prompt row icons', () => {
  it('wears the same box and glyph classes as its neighbours', () => {
    // `renderHeaderAction` gives every action the shared box, and `extraClass`
    // adds the hook the drafts e2e clicks. It reaches the ⋯ row too.
    expect(clearSpec()).toMatch(/extraClass: 'prompt-clear'/);
    expect(headerActionsSource).toMatch(/icon-btn header-icon\$\{a\.extraClass/);
    expect(headerActionsSource).toMatch(/thread-overflow-item\$\{a\.extraClass/);
  });

  it('is measured by the fold', () => {
    // The fold sums every [data-row-item], and a member missing the marker is
    // room the row spends without knowing. The composer stamps one on every
    // member and on the ⋯ trigger. The member's own name goes beside it, so a
    // folded width can still be remembered.
    expect(promptSource).toMatch(
      /const foldAttrs = \(key: string\) => \(\{ 'data-row-item': 'fold', \[FOLD_KEY_ATTR\]: key \}\);/,
    );
    expect(promptSource).toMatch(/triggerAttrs=\{MORE_TRIGGER_ATTRS\}/);
  });

  // It used to render at `visibility: hidden` instead, holding a 2.25rem box in
  // a row that had nothing to clear. On a phone that reservation is what pushed
  // the Diff button off a row that could otherwise hold it.
  it('renders only while there is a draft to clear', () => {
    expect(promptSource).toMatch(/if \(hasText\) \{\n\s+foldActions\.push\(\{\n\s+key: 'prompt-clear'/);
    expect(clearSpec()).not.toMatch(/\binvisible\b/);
  });

  it('declares no size or colour of its own, on any viewport', () => {
    const bodies = [...clearRuleBodies(composerCss), ...clearRuleBodies(mobileCss)];
    for (const body of bodies) {
      expect(
        body,
        `.prompt-clear must inherit .icon-btn.header-icon, but a rule sets: ${body.trim()}`,
      ).not.toMatch(/\b(width|height|color|padding|margin|align-self|font-size)\b/);
    }
  });
});
