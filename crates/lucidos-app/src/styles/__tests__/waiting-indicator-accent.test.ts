import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, join } from 'node:path';
import { cssRules } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
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
    for (const rule of rules) {
      expect(rule.props.get('background'), `${rule.selector} must not fill`).toBeUndefined();
      expect(rule.props.get('border-radius'), `${rule.selector} must not frame`).toBeUndefined();
    }
  });
});
