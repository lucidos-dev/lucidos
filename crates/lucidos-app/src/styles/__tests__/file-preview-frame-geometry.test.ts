/**
 * The file preview's column keeps the preview at exactly the pane's height.
 *
 * A file preview used to be a direct child of `.content-pane-body`, and both
 * roots that can land there size themselves against their parent:
 * `.file-preview-inline` (panels/previews.css) and `.repo-preview-split`
 * (panels/content.css) are each `height: 100%`. Putting a path row above one of
 * them therefore costs exactly that row: the preview becomes a path row TALLER
 * than the pane, so its own inner scroller runs off the bottom and the pane body
 * grows a second scrollbar around it.
 *
 * `.file-preview-frame` is what pays for the row instead. Three declarations do
 * it, and the failure is silent if any one is dropped:
 *
 *   - the frame is the full pane height, so the percentage the preview resolves
 *     against is still the pane;
 *   - the body takes the leftover (`flex: 1`), which is the pane minus the row;
 *   - `min-height: 0`, without which a flex item refuses to shrink below its
 *     content and the body stays pane-height regardless of the row.
 *
 * A source scan rather than a browser test, matching the sibling geometry
 * guards: what regresses here is which declaration is written, and the visible
 * symptom (a nested scrollbar) needs a tall file in a real preview to show up at
 * all. `e2e/repo-files.spec.ts` exercises the preview end to end.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

import { rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const PREVIEWS: string = readFileSync(resolve(here, '../panels/previews.css'), 'utf8');

/** The one rule targeting `className`, with nothing overriding it elsewhere in
 *  the sheet: a second copy under an `@media` would make the scan read a value
 *  the browser does not use. */
function soleRule(className: string) {
  const rules = rulesTargeting(PREVIEWS, className);
  expect(rules, `${className} must be declared exactly once`).toHaveLength(1);
  return rules[0];
}

describe('the file preview column', () => {
  it('is the pane height, so the preview under it still resolves 100% to the pane', () => {
    expect(soleRule('file-preview-frame').props.get('height')).toBe('100%');
  });

  it('gives the preview the leftover height, and lets it shrink to it', () => {
    const body = soleRule('file-preview-frame-body').props;
    expect(body.get('flex')).toBe('1');
    // Without this the item's floor is its content height, so the body stays
    // pane-height and the path row pushes the preview off the bottom.
    expect(body.get('min-height')).toBe('0');
  });

  it('holds the path row at its own height rather than letting it flex', () => {
    expect(soleRule('file-preview-path').props.get('flex')).toBe('0 0 auto');
  });
});

describe('the path row', () => {
  it('breaks a path anywhere, since a path offers no break opportunity of its own', () => {
    // The row exists to show a whole path; ordinary wrapping would find no
    // space in `system-knowhow/workspace-audit.md` and run it out of the pane.
    expect(soleRule('file-preview-path').props.get('overflow-wrap')).toBe('anywhere');
  });

  it('takes the content pane structural gutter from the spacing scale', () => {
    expect(soleRule('file-preview-path').props.get('padding')).toBe('var(--space-sm) var(--space-lg)');
  });
});
