/**
 * Pins the host dropdown trigger's size to the values the user named when
 * this drifted from the app SDK's `lucidos.ui.Select` trigger: 46px tall next
 * to a 30px `.action-btn`, instead of matching it. The structural fix is that
 * `.dropdown-trigger` now lives in ONE place (`shared-components.css`) and
 * `lucidos.ui.Select` renders that exact class — see
 * docs/plans/2026-10-03-sdk-dropdown-shares-host-css.md. This test is the
 * values-level backstop: a future edit to the one shared rule is a
 * deliberate, reviewed change, not a silent drift back to the old numbers.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const SHARED_CSS = readFileSync(
  resolve(here, '../global/shared-components.css'),
  'utf8',
);

describe('.dropdown-trigger (host + lucidos.ui.Select, shared-components.css)', () => {
  const trigger = block(SHARED_CSS, '\n.dropdown-trigger {');

  it('keeps the compact single-line-UI size, not the chat prose step', () => {
    expect(decl(trigger, 'font-size')).toBe('var(--font-size-xs)');
    expect(decl(trigger, 'padding')).toBe('0.25rem 0.5rem');
    expect(decl(trigger, 'border-radius')).toBe('calc(var(--radius-control) * 0.75)');
  });
});
