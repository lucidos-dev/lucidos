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

/** The clear button's own JSX.
 *
 *  Anchored on the ELEMENT CARRYING THE CLASS, then walked outward to its tags.
 *  A single `/<button[\s\S]*?prompt-clear[\s\S]*?<\/button>/` looks equivalent
 *  and is not: it starts at the FIRST `<button` in the file and lazily runs to
 *  the first `prompt-clear` after it, so it returns a span holding every control
 *  in between. The per-attribute assertions below would then pass on some other
 *  button's markup, and this guard would keep reporting green with the attribute
 *  it exists to protect deleted.
 *
 *  The one-tag check is what makes that permanent rather than a property of the
 *  anchoring being right: any span reaching back past another control fails HERE,
 *  loudly, instead of quietly satisfying the assertions below from a neighbour. */
function clearButton(): string {
  const classAttr = /class=\{`[^`]*\bprompt-clear\b[^`]*`\}/.exec(promptSource);
  if (!classAttr) throw new Error('nothing carries the prompt-clear class in PromptInput.tsx');
  const open = promptSource.lastIndexOf('<button', classAttr.index);
  if (open < 0) throw new Error('the prompt-clear element is not inside a <button>');
  const span = promptSource.slice(open, at('</button>', classAttr.index));
  const tags = span.match(/<button\b/g)?.length ?? 0;
  if (tags !== 1) {
    throw new Error(`expected exactly one <button> around the prompt-clear class, found ${tags}`);
  }
  return span;
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

  // Positions are taken from the button's OWN opening tag, never from the first
  // `prompt-clear` in the file: the class is named in prose above the button
  // too, and a prose mention would satisfy these on the strength of a comment.
  it('renders the clear button inside .prompt-actions-row', () => {
    expect(promptSource.indexOf(clearButton()))
      .toBeGreaterThan(at('<div class={rowClass}'));
  });

  it('places the clear button ahead of the right-hand action group', () => {
    // Last of the LEFT cluster, so the right group keeps a single anchor. The
    // right group is `margin-left: auto`, which is what makes the reserved box
    // trailing whitespace while the draft is empty.
    expect(promptSource.indexOf(clearButton()))
      .toBeLessThan(at('<div class={rightClass}>'));
  });
});

describe('the clear button is one of the prompt row icons', () => {
  it('wears the same box and glyph classes as its neighbours', () => {
    expect(clearButton()).toMatch(/class=\{`icon-btn header-icon prompt-clear/);
  });

  it('is measured by the row-overflow hook', () => {
    // useFitsInOneRow sums every [data-row-item]; a control missing the
    // attribute lets the row overflow instead of lifting its liftable slot.
    expect(clearButton()).toMatch(/\bdata-row-item\b/);
  });

  it('keeps its width while hidden, so the row does not jitter when typing', () => {
    expect(clearButton()).toMatch(/hasText \? '' : ' invisible'/);
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
