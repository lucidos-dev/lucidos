/**
 * The welcome surface shares the composer's content box, and the declaration
 * that makes it do so is load-bearing.
 *
 * Both welcome variants wear `.response-content`, which sets `max-width: 100%`.
 * That has the same specificity as the `--content-max-width` cap on
 * `.thread-content > *`, and `chat.css` imports `response.css` AFTER
 * `input-messages.css`, so the 100% wins on source order. Drop the explicit cap
 * on `.welcome-message` and the surface runs the full pane, while the composer
 * under it stays capped and centred. The two then stop sharing an edge.
 *
 * Read as "this rule cannot be deleted as redundant": it looks like a
 * restatement of `.thread-content > *` and is not one.
 */
import { describe, it, expect } from 'vitest';
import postcss, { type Rule } from 'postcss';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const RESPONSE_CSS = resolve(here, '../chat/response.css');
const INPUT_CSS = resolve(here, '../chat/input-messages.css');

/** Declarations of one rule, by selector, in the given stylesheet. */
function declarationsOf(cssPath: string, selector: string): Record<string, string> {
  const root = postcss.parse(readFileSync(cssPath, 'utf8'));
  const out: Record<string, string> = {};
  root.walkRules((rule: Rule) => {
    if (rule.selector.trim() !== selector) return;
    rule.walkDecls((d) => {
      out[d.prop] = d.value.trim();
    });
  });
  return out;
}

describe('.welcome-message shares the composer content box', () => {
  it('caps, centres and insets itself', () => {
    const welcome = declarationsOf(RESPONSE_CSS, '.welcome-message');
    expect(welcome['max-width']).toBe('var(--content-max-width)');
    expect(welcome['margin-left']).toBe('auto');
    expect(welcome['margin-right']).toBe('auto');
    expect(welcome['padding-inline']).toBe('var(--turn-body-inset)');
  });

  it('uses the same cap and inset the composer column does', () => {
    const welcome = declarationsOf(RESPONSE_CSS, '.welcome-message');
    const composer = declarationsOf(INPUT_CSS, '.prompt-input-container');
    // Compared as declarations rather than as a hardcoded value, so a retune of
    // the composer's box has to move the welcome with it.
    expect(welcome['max-width']).toBe(composer['max-width']);
    expect(welcome['padding-inline']).toBe(composer['padding-inline']);
  });

  it('leaves the hero variant no second copy of the box', () => {
    // One owner. A copy on `.welcome-hero` is what let the provider-setup
    // variant, which does not wear that class, run the full pane.
    const hero = declarationsOf(RESPONSE_CSS, '.welcome-hero');
    expect(hero['max-width']).toBeUndefined();
    expect(hero['margin-left']).toBeUndefined();
    expect(hero['padding-inline']).toBeUndefined();
  });

  it('still declares the `.response-content` override this defends against', () => {
    // The guard is only meaningful while the override exists. If someone drops
    // `max-width: 100%` from `.response-content`, this test should be revisited
    // rather than silently guarding nothing.
    const response = declarationsOf(RESPONSE_CSS, '.response-content');
    expect(response['max-width']).toBe('100%');
  });
});
