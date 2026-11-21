/**
 * A ⋯ menu taller than the screen scrolls. The dashboard's Add lists every
 * widget and every app, which on a phone ran off the bottom with no way to
 * reach the rest.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { rulesTargeting } from '../../../styles/__tests__/css-rule-helpers';
import { overflowMenuPanelStyle } from '../OverflowMenu';

const here: string = dirname(fileURLToPath(import.meta.url));
const hostCss: string = readFileSync(resolve(here, '../../../styles/global/host-components.css'), 'utf-8');

describe('a tall overflow menu scrolls', () => {
  it('caps its height at the measured room and scrolls the rest', () => {
    const menu = rulesTargeting(hostCss, 'thread-overflow-menu').find((r) => r.selector === '.thread-overflow-menu');
    expect(menu, 'no .thread-overflow-menu rule').toBeDefined();
    expect(menu?.props.get('max-height')).toContain('var(--anchor-room');
    expect(menu?.props.get('overflow-y')).toBe('auto');
  });

  it('publishes the room the positioning hook measured', () => {
    const style = overflowMenuPanelStyle({ top: 40, left: 8, maxHeight: 610 }) as Record<string, string>;
    expect(style['--anchor-room']).toBe('610px');
    expect(style.top).toBe('40px');
  });

  it('stays hidden until it is placed', () => {
    expect(overflowMenuPanelStyle(null)).toEqual({ visibility: 'hidden' });
  });
});
