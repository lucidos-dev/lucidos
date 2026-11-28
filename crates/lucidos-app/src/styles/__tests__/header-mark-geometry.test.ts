import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { REDUCED_MOTION_ROOT, block, decl, cssRules, isReducedMotionRule, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');

const markCss = styles('header-mark.css');
const mobileCss = styles('mobile.css');
const shellCss = styles('panels/shell.css');
const switcherSource: string = readFileSync(
  resolve(here, '../../components/layout/WorkspaceSwitcher.tsx'),
  'utf-8',
);

const MOBILE = '@media (--phone-layout)';

function mobileRoot(atRules: string): Map<string, string> {
  const found = cssRules(mobileCss).filter(r => r.selector === ':root' && r.atRules === atRules);
  expect(found.length, `expected exactly one \`:root\` in mobile.css at "${atRules}"`).toBe(1);
  return found[0].props;
}

function trailingClusterGap(): string {
  const actions = cssRules(shellCss).find(r => r.selector === '.content-header-actions');
  expect(actions?.props.get('gap'), '.content-header-actions declares no gap').toBeDefined();
  return actions!.props.get('gap')!;
}

describe('the menu paints above a persistent toast', () => {
  it('the panel is rebased above --z-toast', () => {
    expect(decl(block(markCss, '.brand-menu {'), 'z-index')).toBe('calc(var(--z-toast) + 50)');
  });

  it('paints no scrim of its own', () => {
    expect(markCss).not.toMatch(/\.brand-menu-scrim\b/);
  });
});

describe('the unfolded workspace list scrolls instead of overflowing the panel', () => {
  it('.brand-menu-ws-scroll caps its height and scrolls', () => {
    const scroll = block(markCss, '.brand-menu-ws-scroll {');
    expect(decl(scroll, 'max-height')).toBe('var(--brand-menu-ws-list-max-height)');
    expect(decl(scroll, 'overflow-y')).toBe('auto');
  });

  it('Manage workspaces sits outside the scroller, so it never scrolls away', () => {
    const list = block(markCss, '.brand-menu-ws-list {');
    expect(decl(list, 'max-height')).toBeNull();
    expect(decl(list, 'overflow-y')).toBeNull();

    const from = switcherSource.indexOf('<div class="brand-menu-ws-scroll">');
    const to = switcherSource.indexOf('brand-menu-ws-row brand-menu-ws-manage', from);
    expect(from, 'the scroller is gone').toBeGreaterThanOrEqual(0);
    expect(to, 'the manage row is gone').toBeGreaterThan(from);
    const between = switcherSource.slice(from, to);
    expect(between.match(/<\/div>/g)?.length).toBe(between.match(/<div\b/g)?.length);
  });

  it('the panel itself is bounded by the room below the header', () => {
    const panel = block(markCss, '.brand-menu {');
    expect(decl(panel, 'max-height')).toContain('var(--app-header-bottom)');
    expect(decl(panel, 'overflow')).toBe('hidden auto');
  });

  it('the cap is bounded by the viewport, not by rem alone', () => {
    const cap = decl(block(markCss, ':root {'), '--brand-menu-ws-list-max-height');
    expect(cap).toContain('vh');
  });
});

describe('an unfolded action indents off the row it hangs under', () => {
  it('leaves the shared rule with the dressing and no indent of its own', () => {
    const shared = block(markCss, '.brand-menu-ws-action {');
    expect(decl(shared, 'color')).toBe('var(--text-muted)');
    expect(
      decl(shared, 'padding-left'),
      'an indent here would be one host\'s column imposed on both',
    ).toBeNull();
  });

  it('derives each indent from its own host\'s leading column', () => {
    const underDot = decl(block(markCss, '.brand-menu-ws-action-under-dot {'), 'padding-left') ?? '';
    expect(underDot).toContain('var(--ws-picker-dot-size)');
    expect(decl(block(markCss, '.brand-menu-ws-action-under-icon {'), 'padding-left'))
      .toBe('var(--brand-menu-text-column)');
    const underIcon = decl(block(markCss, ':root {'), '--brand-menu-text-column') ?? '';
    expect(underIcon).toContain('var(--icon-size-md)');
    for (const indent of [underDot, underIcon]) {
      expect(indent, 'a raw rem agrees with the row above it at one ui-scale only')
        .toMatch(/var\(--space-(xs|sm|md|lg|xl)\)/);
      expect(indent).not.toMatch(/\d+(\.\d+)?rem/);
    }
  });
});

describe('an activity row\'s detail shares the rows\' margins', () => {
  const body = block(markCss, '.brand-menu-activity-body {');

  it('takes the rows\' own inline padding, so its text starts under their icons', () => {
    const [, inline, , extra] = (decl(body, 'padding') ?? '').split(/\s+/);
    expect(extra, 'a fourth value is a left indent of its own').toBeUndefined();
    expect(inline).toBe(decl(block(markCss, '.brand-menu-item {'), 'padding')?.split(/\s+/)[1]);
  });
});

describe('an activity row keeps its mark on the first line', () => {
  const row = block(markCss, '.brand-menu-activity-row {');
  const line = 'var(--brand-menu-activity-line)';

  it('top-aligns the row, with padding that keeps the shared height', () => {
    expect(decl(row, 'align-items')).toBe('flex-start');
    expect(decl(row, 'padding-block')).toContain('var(--brand-menu-row-min-height)');
    expect(decl(row, '--brand-menu-activity-line')).toContain('var(--brand-menu-row-line-height)');
  });

  it('sizes the mark and the chevron to one line of the label', () => {
    expect(decl(block(markCss, '.brand-menu-activity-mark {'), 'height')).toBe(line);
    expect(decl(block(markCss, '.brand-menu-activity-chevron {'), 'height')).toBe(line);
    expect(decl(block(markCss, '.brand-menu-activity-detail {'), 'line-height')).toBe(line);
  });

  it('draws the spinner in the label\'s colour, not the accent', () => {
    const spinner = block(markCss, '.brand-menu-activity-mark .mini-spinner {');
    expect(decl(spinner, '--spinner-color')).toBe('currentColor');
  });
});

describe('the connection notice is a raised statement, not a row', () => {
  const notice = block(markCss, '.brand-menu-notice {');

  it('lifts on the shared surface, with no frame and no stripe', () => {
    expect(decl(notice, 'background')).toBe('var(--bg-tertiary)');
    for (const frame of ['border', 'border-left', 'box-shadow', 'outline']) {
      expect(decl(notice, frame), `a ${frame} turns the notice into a card`).toBeNull();
    }
    expect(decl(notice, 'border-radius')).toBe('var(--brand-menu-item-radius)');
  });

  it('takes nothing that would promise a tap', () => {
    const targeting = rulesTargeting(markCss, 'brand-menu-notice');
    for (const rule of targeting) {
      expect(rule.props.get('cursor'), `${rule.selector} offers a pointer`).toBeUndefined();
      expect(rule.selector, 'a hover state would make it look like a control')
        .not.toContain(':hover');
    }
  });

  it('lands its dot on the first line, derived rather than nudged', () => {
    expect(decl(notice, 'align-items')).toBe('flex-start');
    const dot = block(markCss, '.brand-menu-notice .status-dot {');
    const offset = decl(dot, 'margin-top') ?? '';
    expect(offset).toContain('var(--brand-menu-row-line-height)');
    expect(offset).toContain('var(--brand-menu-notice-dot)');
    expect(decl(notice, 'line-height')).toBe('var(--brand-menu-row-line-height)');
    expect(decl(dot, 'width')).toBe('var(--brand-menu-notice-dot)');
  });

  it('gets its colour from the shared dot scale, for every state it can show', () => {
    const dotStates = rulesTargeting(shellCss, 'status-dot')
      .flatMap(r => [...r.selector.matchAll(/\.status-dot\.([\w-]+)/g)].map(m => m[1]));
    for (const state of ['connecting', 'disconnected']) {
      expect(dotStates, `.status-dot.${state} has no colour of its own`).toContain(state);
    }
  });
});

describe('mobile header rows share one height', () => {
  it('.mobile-header-row sets a fixed height, not a min-height', () => {
    const row = block(mobileCss, '.mobile-header-row {');
    expect(decl(row, 'height')).toBe('var(--mobile-header-row-height)');
    expect(
      decl(row, 'min-height'),
      'a min-height makes each row as tall as its own tallest control, which is the bug',
    ).toBeNull();
  });

  it('the height token is the tallest control the rows carry', () => {
    expect(mobileRoot(MOBILE).get('--mobile-header-row-height')).toBe('var(--header-mark-tap)');
    expect(decl(block(markCss, ':root {'), '--header-mark-tap')).not.toBeNull();
  });

  it('nothing re-introduces a per-row height override', () => {
    const overrides = rulesTargeting(mobileCss, 'mobile-header-row')
      .filter(r => r.props.has('height') || r.props.has('min-height'))
      .filter(r => r.selector !== '.mobile-header-row');
    expect(
      overrides.map(r => `${r.atRules} ${r.selector}`),
      'a height override on one row is the same bug in a new place',
    ).toEqual([]);
  });
});

describe('the mark says its connection in strength alone', () => {
  it('connected is the header foreground at full light, and nothing else', () => {
    const connected = block(markCss, '.brand-mark[data-conn="connected"] .brand-mark-glyph {');
    expect(
      decl(connected, 'color'),
      'the state the user looks at all day is the brand at full strength',
    ).toBe('var(--header-fg)');
    for (const decoration of ['background', 'border', 'box-shadow', 'outline', 'animation', 'opacity']) {
      expect(decl(connected, decoration), `connected must carry no ${decoration}`).toBeNull();
    }
  });

  it('has no ring pseudo-element on the tile', () => {
    expect(markCss).not.toMatch(/\.brand-mark-glyph::(after|before)/);
  });

  it('states the two receded states as opacity only', () => {
    const disconnected = block(
      markCss,
      '.brand-mark[data-conn="disconnected"] .brand-mark-glyph,\n.brand-mark[data-conn="stopped"] .brand-mark-glyph {',
    );
    expect(decl(disconnected, 'opacity')).toBe('var(--header-mark-disconnected-opacity)');
    expect(decl(disconnected, 'color')).toBeNull();
    expect(decl(disconnected, 'background')).toBeNull();

    const connecting = block(
      markCss,
      '.brand-mark[data-conn="connecting"] .brand-mark-glyph,\n.brand-mark[data-conn="starting"] .brand-mark-glyph,\n'
        + '.brand-mark[data-conn="restarting"] .brand-mark-glyph,\n.brand-mark[data-conn="switching-version"] .brand-mark-glyph {',
    );
    expect(decl(connecting, 'animation')).toContain('brand-mark-breathe');
    expect(decl(connecting, 'color')).toBeNull();
  });

  it('reduced motion actually beats the animation it is cancelling', () => {
    const markRules = cssRules(markCss).filter(r => r.selector.startsWith('.brand-mark'));
    const animated = markRules.filter(r => r.props.has('animation') && r.props.get('animation') !== 'none');
    expect(animated.map(r => r.selector), 'the waiting states are the only animated ones').toEqual([
      '.brand-mark[data-conn="connecting"] .brand-mark-glyph, .brand-mark[data-conn="starting"] .brand-mark-glyph, '
        + '.brand-mark[data-conn="restarting"] .brand-mark-glyph, .brand-mark[data-conn="switching-version"] .brand-mark-glyph',
    ]);

    const cancels = cssRules(markCss).filter(r =>
      isReducedMotionRule(r) && r.props.get('animation') === 'none'
      && r.selector.slice(`${REDUCED_MOTION_ROOT} `.length).startsWith('.brand-mark'));
    const unprefixed = (selector: string) => selector.split(`${REDUCED_MOTION_ROOT} `).join('');
    expect(
      cancels.map(r => unprefixed(r.selector)),
      'the reduce override must repeat the animated selector, or it loses on specificity',
    ).toEqual(animated.map(r => r.selector));

    expect(cancels[0].props.get('opacity')).toBe('var(--header-mark-connecting-opacity-min)');
  });

  it('renders one variant, so both panes show the same mark', () => {
    for (const dead of ['brand-mark-brand', 'brand-mark-muted']) {
      expect(markCss.includes(dead), `${dead} was replaced by the single plain rendering`).toBe(false);
    }
  });

  it('the threads row reads at the same strength, and outranks the bar to do it', () => {
    const row = cssRules(markCss).find(r => r.selector.includes('.brand-mark-row')
      && r.props.has('color'));
    expect(row?.selector, 'a bare .icon-btn.brand-mark-row loses to the bar')
      .toBe('.app-header .icon-btn.brand-mark-row');
    expect(row?.props.get('color')).toBe('var(--header-fg)');
  });
});

describe('the press cue changes a transform value, never creates one', () => {
  const rule = (selector: string) => {
    const found = cssRules(markCss).filter(r => r.selector === selector);
    expect(found.length, `expected exactly one \`${selector}\` rule`).toBe(1);
    return found[0].props;
  };

  it('the glyph is always transformed, so the press interpolates a matrix', () => {
    const glyph = rule('.brand-mark-glyph');
    expect(glyph.get('transform'), 'and it must be the identity, so it changes no geometry')
      .toBe('scale(1)');
    expect(glyph.get('will-change'), 'the layer must exist before the finger lands')
      .toBe('transform');
  });

  it('and the pressed state is a value on the same property', () => {
    expect(rule('.brand-mark:active .brand-mark-glyph').get('transform')).toBe('scale(0.93)');
    expect(rule('.brand-mark-glyph').get('transition'),
      'the transform must still be the animated property')
      .toContain('transform var(--duration-fast)');
  });
});

describe('the badge rides the mark rather than sitting beside it', () => {
  const badge = block(markCss, '.brand-mark-slot .badge.brand-badge {');

  it('is out of flow, so it cannot widen the slot', () => {
    expect(decl(badge, 'position')).toBe('absolute');
    expect(decl(badge, 'margin'), 'the superscript margins must be cleared').toBe('0');
  });

  it('takes the GLYPH corner, derived, not the tap target corner', () => {
    expect(decl(block(markCss, '\n.brand-mark-slot {'), '--brand-badge-corner'))
      .toBe('calc((100% - var(--header-mark-size)) / 2)');
    for (const side of ['top', 'right']) {
      expect(decl(badge, side), `${side} must derive the corner, not sit at 0`)
        .toBe('calc(var(--brand-badge-corner) + var(--header-mark-badge-offset))');
    }
    expect(decl(block(markCss, ':root {'), '--header-mark-badge-offset')).toBe('0rem');
  });

  it('leaves the mark\'s sparkle alone: the unread count takes the OTHER corner', () => {
    const count = block(markCss, '.brand-mark-slot .badge.brand-unread-badge {');
    expect(decl(count, 'bottom'), 'the count must ride the bottom corner')
      .toBe('calc(var(--brand-badge-corner) + var(--header-mark-badge-offset))');
    expect(decl(count, 'top'), 'the base badge top must be released').toBe('auto');
    expect(decl(badge, 'bottom'), 'the state badge must stay on the top corner').toBeNull();
    expect(decl(count, 'pointer-events')).toBe('none');
  });

  it('stays inside the host box, so no clip can shave it and no measure sees it', () => {
    expect(decl(badge, 'transform'), 'the badge must not overflow its slot').toBeNull();
  });

  it('names the artwork inset once, and both compensations read it', () => {
    expect(decl(block(markCss, ':root {'), '--header-mark-art-scale')).toBe('0.74');
    expect(decl(block(markCss, '.brand-menu-version > svg {'), 'transform'))
      .toBe('scale(calc(1 / var(--header-mark-art-scale)))');
    const icons = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf-8');
    expect(icons, 'the mark no longer carries the inset the token restates')
      .toContain('scale(0.74)');
  });

  it('lets a tap through the state badge in every state, so it reaches the mark', () => {
    const base = block(markCss, '.brand-mark-slot .badge.brand-badge {');
    expect(decl(base, 'pointer-events')).toBe('none');
    expect(markCss, 'no badge variant may take its tap back').not.toContain('brand-badge-action');
  });

  it('beats both sheets that would re-corner it, on specificity', () => {
    const targeting = rulesTargeting(markCss, 'brand-badge').map(r => r.selector);
    expect(targeting).toContain('.brand-mark-slot .badge.brand-badge');
  });
});

describe('the nav chevrons are pinned to one shared span', () => {
  const cluster = block(markCss, '.header-nav-cluster {');

  it('the cluster is a fixed-width centred box', () => {
    expect(decl(cluster, 'position')).toBe('absolute');
    expect(decl(cluster, 'justify-content')).toBe('space-between');
    expect(decl(cluster, 'width')).toBe('var(--header-nav-cluster-width)');
    const width = decl(block(markCss, ':root {'), '--header-nav-cluster-width') ?? '';
    for (const token of ['--header-nav-min-span', '--header-nav-edge-reserve']) {
      expect(width, 'the width must be bounded, or it overlaps the edge clusters at high ui-scale')
        .toContain(token);
    }
  });

  it('holds the chevrons at no fixed distance apart, only against the edges', () => {
    const width = decl(block(markCss, ':root {'), '--header-nav-cluster-width') ?? '';
    expect(width, 'the cluster takes what the row leaves, floored').toMatch(/^max\(/);
    expect(width, 'a third clamp arm is a fixed distance the reserve cannot beat')
      .not.toContain('clamp(');
    expect(markCss, '--header-nav-span is a fixed distance apart, which mobile does not want')
      .not.toContain('--header-nav-span:');
  });

  it('the cluster SPANS the row vertically, so its own height cannot place it', () => {
    expect(decl(cluster, 'top')).toBe('0');
    expect(decl(cluster, 'bottom')).toBe('0');
    expect(decl(cluster, 'height'), 'the inset pair is what spans it; a height re-opens the rounding')
      .toBeNull();
    expect(decl(cluster, 'transform'), 'no transform: a half-pixel one splits the badge in WebKit')
      .toBeNull();
    expect(decl(cluster, 'left'), 'centred by `left` alone, at the same spot')
      .toBe('calc(50% - var(--header-nav-cluster-width) / 2)');
    expect(decl(cluster, 'align-items'), 'flexbox owns the vertical placement now').toBe('center');
  });

  it('the threads pane hangs its mark off the same span, at the trailing edge', () => {
    const end = block(markCss, '.header-mark-end-cluster {');
    expect(decl(end, 'justify-content')).toBe('flex-end');
    for (const own of ['width', 'max-width', 'transform', 'left', 'right']) {
      expect(decl(end, own), `a ${own} here would move the mark off the chevron column`).toBeNull();
    }
  });

  it('the threads title reserve is derived from that span, not from a rem constant', () => {
    const title = block(mobileCss, '.mobile-header-title {');
    const reserve = decl(title, 'max-width') ?? '';
    expect(reserve, 'the reserve must read the cluster it has to clear')
      .toContain('var(--header-nav-cluster-width)');
    expect(reserve, "and the mark's own button box").toContain('var(--mobile-header-icon-box)');
    expect(
      decl(block(mobileCss, '.mobile-header-row .icon-btn.header-icon {'), 'width'),
      'the buttons and the reserve must read one box size, or they disagree',
    ).toBe('var(--mobile-header-icon-box)');
  });

  it('the content span clears edge controls without the brand floor', () => {
    const title = block(markCss, '.header-title-cluster {');
    expect(decl(title, '--header-nav-cluster-width')).toBe(
      'max(0rem, calc(100% - 2 * var(--header-nav-edge-reserve)))',
    );
    expect(decl(title, 'left'), 'inherit the shared centered position').toBeNull();
    expect(decl(title, '--content-title-reclaim')).toBeNull();
    expect(decl(title, 'width')).toBeNull();
    expect(decl(title, 'max-width')).toBeNull();
    expect(decl(title, 'transform')).toBeNull();
  });

  it('the empty span cannot swallow a tap aimed at the row underneath', () => {
    expect(decl(cluster, 'pointer-events')).toBe('none');
    const restore = cssRules(markCss)
      .filter(r => /\.header-nav-cluster > \*$/.test(r.selector))
      .filter(r => r.props.get('pointer-events') === 'auto');
    expect(
      restore.length,
      'the members must get their events back, or nothing in the cluster is tappable',
    ).toBe(1);

    for (const state of ['data-keyboard-active', 'data-overlay-open']) {
      expect(
        restore[0].selector,
        `the restore must not apply under [${state}], whose inert only inherits`,
      ).toContain(`:not([${state}])`);
    }
  });

  it('the chevrons and the mark cannot be squeezed by a long title', () => {
    const fixed = cssRules(markCss).find(r => r.selector.includes('.header-nav-cluster > .icon-btn'));
    expect(fixed?.props.get('flex')).toBe('0 0 auto');
    expect(fixed?.selector).toContain('.header-nav-cluster > .header-mark-group');
  });

  it('the floor is the thread row\'s narrowest width, the whole group included', () => {
    const floor = decl(block(markCss, ':root {'), '--header-nav-min-span') ?? '';
    expect(floor, 'two chevrons, Home and the dashboard toggle').toContain('4 * var(--mobile-header-icon-box)');
    for (const term of ['--header-mark-tap', '--header-nav-gap']) {
      expect(floor, `the floor must count ${term}`).toContain(`var(${term})`);
    }
    expect(floor, 'the floor must not count the group gap').not.toContain('--header-mark-group-gap');
  });

  it('the group centres the mark, spreads out when there is room, and closes up when there is not', () => {
    const group = cssRules(markCss)
      .filter(r => r.selector === '.header-nav-cluster > .header-mark-group')
      .find(r => r.props.has('grid-template-columns'));
    expect(group, 'the group rule is gone').toBeDefined();
    expect(group!.props.get('grid-template-columns')).toBe('1fr auto 1fr');
    const flex = group!.props.get('flex') ?? '';
    expect(flex, 'it may shrink but never grow').toMatch(/^0 1 calc\(/);
    for (const term of ['--mobile-header-icon-box', '--header-mark-tap', '--header-mark-group-gap-max']) {
      expect(flex, `the basis must count ${term}`).toContain(`var(${term})`);
    }
    const fixedGaps = cssRules(markCss)
      .filter(r => r.selector.includes('.header-mark-group') && r.props.has('gap'));
    expect(fixedGaps, 'a fixed gap cannot close up, and it would stack on the spread').toEqual([]);
  });
});

describe('the nav chevrons reach past their button for a tap', () => {
  const rule = (selector: string): Map<string, string> => {
    const found = cssRules(markCss).find(r => r.selector === selector);
    expect(found, `${selector} is gone`).toBeDefined();
    return found!.props;
  };

  it('the target is at least 44px at the 16px root', () => {
    const target = decl(block(markCss, ':root {'), '--header-nav-hit-target') ?? '';
    expect(target).toMatch(/^[\d.]+rem$/);
    expect(parseFloat(target) * 16).toBeGreaterThanOrEqual(44);
  });

  it('is an out-of-flow overlay derived from the button box', () => {
    const reach = rule('.header-nav-cluster > .nav-chevron::before');
    expect(reach.get('content')).toBe("''");
    expect(reach.get('position')).toBe('absolute');
    expect(reach.get('--nav-chevron-reach'))
      .toBe('calc((var(--header-nav-hit-target) - var(--mobile-header-icon-box)) / 2)');
    for (const side of ['top', 'bottom']) {
      expect(reach.get(side), `the reach is centred on the ${side} axis`).toBe('calc(-1 * var(--nav-chevron-reach))');
    }
  });

  it('loses every overlap, because its neighbours are raised over it', () => {
    expect(rule('.header-nav-cluster > .nav-chevron::before').get('z-index')).toBeUndefined();
    const inside = rule(
      '.header-nav-cluster > .brand-mark-slot, .header-nav-cluster > .header-mark-group, '
      + '.header-title-cluster > .mobile-content-title',
    );
    expect(inside.get('position')).toBe('relative');
    expect(inside.get('z-index')).toBe('1');
    expect(rule('.mobile-header-row > .icon-btn').get('z-index')).toBe('1');
  });

  it('reaches twice as far toward the middle as toward the row edge', () => {
    const back = rule('.header-nav-cluster > .nav-chevron:first-child::before');
    const forward = rule('.header-nav-cluster > .nav-chevron:last-child::before');
    const outer = 'calc(-1 * var(--nav-chevron-reach))';
    const inner = 'calc(-2 * var(--nav-chevron-reach))';
    expect([back.get('left'), back.get('right')]).toEqual([outer, inner]);
    expect([forward.get('left'), forward.get('right')]).toEqual([inner, outer]);
  });
});

describe('the nav reserve is the widest edge cluster, and no wider', () => {
  const reserve = (): string => {
    const value = decl(block(markCss, ':root {'), '--header-nav-edge-reserve');
    expect(value, '--header-nav-edge-reserve is gone').not.toBeNull();
    return value!;
  };

  it('is two icon boxes and the one gap between them', () => {
    expect(reserve()).toBe('calc(2 * var(--mobile-header-icon-box) + 0.25rem)');
  });

  it('counts the trailing cluster\'s OWN gap, not a number that once matched it', () => {
    const gap = trailingClusterGap();
    expect(
      /\+ ([\d.]+rem)\)$/.exec(reserve())?.[1],
      `the reserve drifted from the trailing cluster's ${gap} gap`,
    ).toBe(gap);
  });

  it('clears that cluster exactly, so the chevron takes the slot inboard of it', () => {
    const box = parseFloat(mobileRoot('').get('--mobile-header-icon-box')!);
    const gap = parseFloat(trailingClusterGap());
    const boxes = Number(/calc\((\d+) \* var\(--mobile-header-icon-box\)/.exec(reserve())?.[1]);
    const reserveRem = boxes * box + parseFloat(/\+ ([\d.]+rem)\)$/.exec(reserve())![1]);
    const widestCluster = 2 * box + gap;
    expect(reserveRem, 'the widest cluster would overlap the chevron').toBeGreaterThanOrEqual(widestCluster);
    expect(reserveRem, 'a whole icon box of clearance pushes the chevrons in')
      .toBeLessThan(widestCluster + box);
  });

  it('names a box declared for every width, or the clamp goes invalid', () => {
    expect(
      mobileRoot('').get('--mobile-header-icon-box'),
      'the box must be declared outside the mobile breakpoint',
    ).toBe('1.75rem');
    expect(
      mobileRoot(MOBILE).get('--mobile-header-icon-box'),
      'a second declaration inside the breakpoint is a copy that can drift',
    ).toBeUndefined();
  });
});

describe('the desktop brand span pays for the same pattern the same way', () => {
  it('none on the box, auto on the members, and the restore stands down behind an overlay', () => {
    const label = cssRules(shellCss).find(
      r => r.selector === '.app-header .pane-header-brand .pane-header-brand-label',
    );
    expect(label?.props.get('pointer-events'), 'the span must pass clicks through').toBe('none');

    const restore = cssRules(shellCss)
      .filter(r => /\.pane-header-brand-label > \*$/.test(r.selector))
      .filter(r => r.props.get('pointer-events') === 'auto');
    expect(
      restore.length,
      'the members must get their events back, or nothing in the brand is clickable',
    ).toBe(1);
    expect(
      restore[0].selector,
      'the restore must not apply under [data-overlay-open], whose inert only inherits',
    ).toContain(':not([data-overlay-open])');
  });
});

describe('the deleted collapse measurement left nothing behind', () => {
  it('no stylesheet reads a property nothing publishes any more', () => {
    const orphans = ['--mobile-content-title-max', '--mobile-content-title-shift'];
    const sheets: string[] = [];
    const walk = (dir: string): void => {
      for (const entry of readdirSync(dir, { withFileTypes: true }) as Array<{ name: string; isDirectory(): boolean }>) {
        const full = resolve(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (entry.name.endsWith('.css')) sheets.push(full);
      }
    };
    walk(stylesDir);
    for (const sheet of sheets) {
      const css = readFileSync(sheet, 'utf-8');
      for (const orphan of orphans) {
        expect(css.includes(`var(${orphan}`), `${sheet} still reads ${orphan}`).toBe(false);
      }
    }
  });
});
