/**
 * The dashboard rail's chrome reads like the panes beside it. Its toggle is one
 * control that holds the window's corner in both rail states.
 *
 * Source scans, like `header-drawer-toggle-pinned.test.ts`: a fade, a second
 * position property or a hairline is a fact of the sheet.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, styleSheetPaths, type CssRule } from './css-rule-helpers';

const stylesDir: string = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');

const railCss = styles('panels/dashboard.css');
const shellCss = styles('panels/shell.css');

/** The one rule with this exact selector. */
function rule(css: string, selector: string): CssRule {
  const found = cssRules(css).filter(r => r.selector === selector);
  expect(found.length, `expected exactly one \`${selector}\` rule`).toBe(1);
  return found[0];
}

describe('the rail reads like the panes', () => {
  it('draws its separator with the pane divider\'s width and colour', () => {
    const split = rule(shellCss, '.split-divider').props;
    const band = rule(railCss, '.dashboard-rail-divider').props;
    expect(band.get('width')).toBe(split.get('width'));
    // One solid colour from the window's top edge down: no clear header stretch.
    expect(band.get('background')).toBe(split.get('background'));
    expect(band.get('top')).toBe('0');
    expect(band.get('bottom')).toBe('0');
  });

  it('wears the header icons\' colour on its toggle while docked on the bar', () => {
    const colour = cssRules(shellCss).find(
      r => r.selector.includes('.dashboard-rail-toggle-slot .icon-btn') && r.props.has('color'),
    );
    expect(colour?.props.get('color')).toBe('var(--header-fg-muted)');
    // Shown, the toggle sits on the rail's own head, not the blue bar.
    expect(colour?.selector).toContain(':not([data-dashboard-rail])');
  });

  it('draws no hairline of its own on its edge or under its head', () => {
    for (const r of rulesTargeting(railCss, 'dashboard-rail')) {
      expect(r.props.has('border-left'), `${r.selector} { border-left }`).toBe(false);
    }
    for (const r of rulesTargeting(railCss, 'dashboard-head')) {
      expect(r.props.has('border-bottom'), `${r.selector} { border-bottom }`).toBe(false);
    }
  });
});

describe('a window-framed reader of the divider leaves the rail\'s room out', () => {
  // In the header, `--divider-x`'s 100% is the header, which the rail already
  // shrank. In a `position: fixed` box it is the whole window.
  it('takes the rail\'s share off --divider-x for the window frame', () => {
    const root = cssRules(shellCss).find(r => r.selector === ':root' && r.props.has('--divider-x'))!.props;
    expect(root.get('--divider-x-in-window')).toBe('calc(var(--divider-x) - var(--sr) * var(--dashboard-rail-room))');
    expect(rule(railCss, ':root').props.get('--dashboard-rail-room')).toBe('0px');
    expect(rule(railCss, ':root[data-dashboard-rail]').props.get('--dashboard-rail-room')).toBe('var(--dashboard-rail-width)');
    expect(rule(railCss, '.app-shell').props.get('padding-right')).toBe('var(--dashboard-rail-room)');
  });

  it('is read by every rule outside the header\'s own regions', () => {
    const readers = styleSheetPaths(stylesDir)
      .filter(path => !path.endsWith('panels/shell.css'))
      .flatMap(path => cssRules(readFileSync(path, 'utf-8'))
        .filter(r => [...r.props.values()].some(v => /var\(--divider-x\)/.test(v)))
        .map(r => `${path}: ${r.selector}`));
    expect(readers).toEqual([]);
  });
});

describe('the rail toggle is one control in the window corner', () => {
  const slotRules = (): CssRule[] => rulesTargeting(railCss, 'dashboard-rail-toggle-slot');

  it('holds the window corner whether the rail shows or not, and never fades', () => {
    const base = rule(railCss, '.dashboard-rail-toggle-slot').props;
    expect(base.get('position')).toBe('fixed');
    expect(base.get('right')).toBe('var(--header-padding-x)');
    for (const r of slotRules()) {
      expect(r.props.has('opacity'), `${r.selector} { opacity }`).toBe(false);
      expect(r.props.has('left'), `${r.selector} { left }`).toBe(false);
      if (r.selector !== '.dashboard-rail-toggle-slot') expect(r.props.has('right'), `${r.selector} { right }`).toBe(false);
    }
  });

  it('ends the rail\'s head, which keeps room for it', () => {
    const head = rule(railCss, '.dashboard-rail .dashboard-head').props;
    const padding = (head.get('padding') ?? '').replace(/\s+/g, ' ');
    expect(padding).toContain('max( calc(var(--header-padding-x) + var(--header-icon-box) + var(--space-sm)),');
  });

  it('rests where the bell row keeps room for it while the rail is hidden', () => {
    const docked = rule(
      shellCss,
      ':root[data-dashboard-rail-toggle]:not([data-dashboard-rail]) .app-header .content-header-elements',
    ).props;
    // Padding, never a moved `right`, so the centred title holds still.
    expect(docked.has('right')).toBe(false);
    expect(docked.get('padding-right')).toBe('calc(var(--header-icon-box) + var(--pane-header-gap))');
  });

  it('grows both content reserves by its box and one gap while docked', () => {
    const terms = (value: string | undefined) => {
      const m = /calc\((\d+) \* var\(--header-icon-box\) \+ (\d+) \* 0\.25rem\)/.exec(value ?? '');
      return m ? [Number(m[1]), Number(m[2])] : null;
    };
    const root = cssRules(shellCss).find(r => r.selector === ':root' && r.props.has('--content-side-reserve'))!.props;
    const docked = rule(shellCss, ':root[data-dashboard-rail-toggle]:not([data-dashboard-rail])').props;
    for (const token of ['--content-side-reserve', '--content-unfolded-reserve']) {
      const [boxes, gaps] = terms(root.get(token))!;
      expect(terms(docked.get(token)), token).toEqual([boxes + 1, gaps + 1]);
    }
  });

  it('never shrinks away, so a hidden Canvas never takes it with it', () => {
    for (const r of slotRules()) {
      expect(r.props.get('width'), `${r.selector} { width }`).not.toBe('0');
      expect(r.props.has('visibility'), `${r.selector} { visibility }`).toBe(false);
      expect(r.props.has('transition'), `${r.selector} { transition }`).toBe(false);
    }
  });
});

describe('with the Canvas hidden, the toggle takes a slot in the Conversation row', () => {
  const DOCKED_IN_CHAT = ':root[data-dashboard-rail-toggle][data-content-collapsed]:not([data-dashboard-rail])';

  it('keeps room at the row\'s trailing end, which the action collapse reads', () => {
    const row = rule(shellCss, `${DOCKED_IN_CHAT} .app-header .pane-header-brand`).props;
    // Padding, as the Canvas row does: the centred brand holds still, and
    // `useHeaderActionCollapse` folds the actions short of it.
    expect(row.has('right')).toBe(false);
    // The row runs to the header's edge, so it also keeps the padding the
    // Canvas row stops short by.
    expect(rule(shellCss, '.app-header .content-header-elements').props.get('right')).toBe('var(--header-padding-x)');
    expect(row.get('padding-right')).toBe('calc(var(--header-padding-x) + var(--header-icon-box) + var(--pane-header-gap))');
  });

  it('paints above the Conversation row, which reaches the corner and would swallow its clicks', () => {
    expect(rule(railCss, '.dashboard-rail-toggle-slot').props.get('z-index')).toBe('calc(var(--z-control-panel) + 1)');
  });

  it('slides the actions over with the pane, not after it', () => {
    const transition = rule(shellCss, '.app-header .pane-header-brand').props.get('transition') ?? '';
    expect(transition).toContain('padding-right var(--duration-slow) ease');
  });

  it('widens the brand\'s trailing reserve to the slot plus the actions at their widest', () => {
    const terms = (value: string | undefined) => {
      const m = /calc\((.+?) \+ (\d+) \* var\(--header-icon-box\) \+ (?:(\d+) \* )?0\.25rem\)/.exec(value ?? '');
      return m ? { pad: m[1], boxes: Number(m[2]), gaps: Number(m[3] ?? 1) } : null;
    };
    const root = cssRules(shellCss).find(r => r.selector === ':root' && r.props.has('--brand-trail-end'))!.props;
    const docked = rule(shellCss, DOCKED_IN_CHAT).props;
    const { boxes, gaps } = terms(root.get('--brand-trail-end'))!;
    // The row's padding is the header's own, as the slot's padding above says.
    expect(terms(docked.get('--brand-trail-end'))).toEqual({ pad: 'var(--header-padding-x)', boxes: boxes + 1, gaps: gaps + 1 });
  });
});
