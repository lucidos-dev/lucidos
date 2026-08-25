/**
 * The waiting indicator is accent, and it is accent unconditionally.
 *
 * The prompt row's other two state-bearing controls light up when they are on:
 * the follow toggle at `.active`, the todo indicator at `in-progress`. This one
 * has no on-state to hang a class on. It renders only while the thread is
 * parked, so the paint sits on the bare `data-role` instead. A `.active`
 * creeping into the selector would silence the button, since nothing ever sets
 * that class on it.
 *
 * Two things nothing else in the gate can see, which is why this scan exists.
 * `tsc` never reads CSS, and `vite build` fails only on syntax, so a selector
 * that matches no element builds perfectly clean. The token matters as much as
 * the colour, too. `--accent-notable` is the neutral tone a Waiting STATUS
 * wears in the Info popover. Reach for it here and the armed control would
 * speak in the status dot's voice.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, join } from 'node:path';
import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
/** `crates/lucidos-app/src/styles/`, from `src/styles/__tests__/`. */
const STYLES = resolve(here, '..');
const INDICATOR = '[data-role="waiting-indicator"]';

const promptCss = readFileSync(join(STYLES, 'chat', 'input-messages.css'), 'utf8');
const rules = cssRules(promptCss).filter((r) => r.selector.includes(INDICATOR));

describe('the waiting indicator', () => {
  it('paints the accent', () => {
    const accented = rules.filter((r) => r.props.get('color') === 'var(--accent)');
    expect(accented.length, `one rule for ${INDICATOR} must set color: var(--accent)`).toBe(1);
  });

  it('needs no class, so the paint cannot be silenced by a missing state', () => {
    for (const rule of rules) {
      expect(rule.selector, 'the accent must not be gated on a state class').not.toContain('.active');
    }
  });

  it('carries no frame, so it reads as a mode and not as a jammed-in button', () => {
    // The follow toggle's shape, not the WIP preview toggle's. The reasoning
    // lives on the follow toggle's own rule in `chat/input-messages.css`.
    for (const rule of rules) {
      expect(rule.props.get('background'), `${rule.selector} must not fill`).toBeUndefined();
      expect(rule.props.get('border-radius'), `${rule.selector} must not frame`).toBeUndefined();
    }
  });
});
