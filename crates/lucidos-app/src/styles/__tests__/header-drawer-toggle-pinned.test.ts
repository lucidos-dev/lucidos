import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, focusRingWidthRem, rulesTargeting, selectorList, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');
const component = (rel: string): string => readFileSync(resolve(stylesDir, '../components', rel), 'utf-8');

const shellCss = styles('panels/shell.css');
const shellRules = cssRules(shellCss);
const appHeader = component('layout/AppHeader.tsx');

const DESKTOP = '@media (--desktop-layout)';

function desktopRule(selector: string): CssRule {
  const found = shellRules.filter(r => r.selector === selector && r.atRules === DESKTOP);
  expect(found.length, `expected exactly one desktop \`${selector}\` rule`).toBe(1);
  return found[0];
}

describe('one toggle, in one host', () => {
  it('the desktop header mounts exactly one', () => {
    const mounts = appHeader.match(/<ThreadToggleButton\b/g) ?? [];
    expect(mounts.length, 'a second mount is a crossfading pair waiting to happen').toBe(1);
    expect(appHeader).toContain('class="thread-toggle-slot"');
    expect(appHeader).not.toContain('thread-nav-group');
    expect(shellCss).not.toContain('thread-nav-group');
  });

  it('the third copy, which could never render on any client, is gone', () => {
    expect(component('layout/ThreadPane.tsx')).not.toContain('ThreadToggleButton');
    for (const rel of ['chat/input-messages.css', 'mobile.css']) {
      expect(styles(rel), `${rel} still hides a toggle that is not rendered`)
        .not.toContain('thread-pane-toggle');
    }
  });

  it('reads in the order it sits on screen: toggle, Filter, title, Grouping, Search', () => {
    const at = (needle: string) => {
      const i = appHeader.indexOf(needle);
      expect(i, `${needle} not found in AppHeader.tsx`).toBeGreaterThanOrEqual(0);
      return i;
    };
    const threadsHeader = at('function ThreadsHeader');
    expect(at('class="thread-toggle-slot"')).toBeLessThan(at('<ThreadsHeader />'));
    const filter = appHeader.indexOf('<ThreadFilterButton', threadsHeader);
    const title = appHeader.indexOf('class="threads-header-title"', threadsHeader);
    const grouping = appHeader.indexOf('<ThreadGroupingButton', threadsHeader);
    const search = appHeader.indexOf('aria-label="Search threads"', threadsHeader);
    expect(filter).toBeGreaterThan(threadsHeader);
    expect(filter).toBeLessThan(title);
    expect(title).toBeLessThan(grouping);
    expect(grouping).toBeLessThan(search);
  });

  it('the host has no focus handler: the button fills it and keeps its clicks', () => {
    expect(appHeader).toContain('<div class="thread-toggle-slot">');
  });
});

describe('it rests in the corner in both drawer states', () => {
  it('no rule gives it a drawer-open position', () => {
    for (const rule of rulesTargeting(shellCss, 'thread-toggle-slot')) {
      expect(rule.selector, `${rule.selector} moves the toggle with the drawer`)
        .not.toContain('data-thread-drawer-open');
    }
    expect(desktopRule('.thread-toggle-slot').props.get('left')).toBe('var(--header-lead-inset)');
  });

  it('no rule fades it, in any state', () => {
    const rules = rulesTargeting(shellCss, 'thread-toggle-slot');
    expect(rules.length, 'the slot lost its rules').toBeGreaterThanOrEqual(2);
    for (const rule of rules) {
      expect(rule.props.get('opacity'), `${rule.selector} { opacity }`).toBeUndefined();
      expect(rule.props.get('transition') ?? '', `${rule.selector} { transition }`)
        .not.toContain('opacity');
    }
  });

  it('the drawer row keeps the toggle\'s box free, and clips at its edge', () => {
    const slot = desktopRule('.thread-toggle-slot');
    const row = desktopRule('.threads-header');
    expect(row.props.get('--threads-row-lead')).toBe(
      `calc(${slot.props.get('left')} + ${slot.props.get('width')} + var(--pane-header-gap))`,
    );
    expect(row.props.get('padding')).toMatch(/ var\(--threads-row-lead\)$/);
    expect(row.props.get('clip-path')).toMatch(/ var\(--threads-row-lead\)\)$/);
  });
});

describe('a Conversation-pane collapse shrinks it away', () => {
  it('it leaves with its pane instead of fading where the hamburger lands', () => {
    const rule = desktopRule(':root[data-thread-collapsed] .thread-toggle-slot');
    expect(rule.props.get('width')).toBe('0');
    expect(rule.props.get('visibility')).toBe('hidden');
    expect(rule.props.get('pointer-events')).toBe('none');
  });

  it('`left` is declared once, at the corner, and nothing transitions it', () => {
    const lefts = rulesTargeting(shellCss, 'thread-toggle-slot')
      .filter(r => r.props.has('left'))
      .map(r => `${r.selector} { left: ${r.props.get('left')} }`);
    expect(lefts).toEqual(['.thread-toggle-slot { left: var(--header-lead-inset) }']);
    expect(desktopRule('.thread-toggle-slot').props.get('transition')).not.toMatch(/\bleft\b/);
  });

  it('`width` is what transitions, and a pane resize kills it', () => {
    const base = desktopRule('.thread-toggle-slot');
    expect(base.props.get('position')).toBe('absolute');
    expect(base.props.get('transition')).toContain('width var(--duration-slow) ease');
    expect(base.props.get('transition')).toContain('visibility var(--duration-slow)');
    const killed = shellRules.find(
      r => r.atRules === DESKTOP && r.props.get('transition') === 'none'
        && r.selector.includes('[data-pane-resizing]'),
    );
    expect(killed?.selector, 'the pane-resize kill list is gone')
      .toContain(':root[data-pane-resizing] .thread-toggle-slot');
  });

  it('the slot clips at one constant edge, clear of the focus ring', () => {
    const clip = desktopRule('.thread-toggle-slot').props.get('clip-path') ?? '';
    const slack = -parseFloat(/^inset\((-[\d.]+)rem\)$/.exec(clip)?.[1] ?? 'NaN');
    const band = focusRingWidthRem(styles('global/base.css'));
    expect(slack, `clip ${clip} cuts the ${band}rem focus ring`).toBeGreaterThanOrEqual(band);
    for (const rule of rulesTargeting(shellCss, 'thread-toggle-slot')) {
      expect(rule.props.get('overflow'), `${rule.selector} { overflow }`).toBeUndefined();
      if (rule.selector === '.thread-toggle-slot') continue;
      expect(rule.props.get('clip-path'), `${rule.selector} swaps the clip`).toBeUndefined();
    }
  });

  it('the whole-region collapse fade does not reach it', () => {
    for (const rule of shellRules) {
      if (rule.props.get('opacity') !== '0') continue;
      expect(rule.selector, `${rule.selector} { opacity: 0 }`).not.toContain('thread-toggle-slot');
    }
  });

  it('the slot\'s stated width is the button\'s own box, in both axes', () => {
    const iconBox = cssRules(styles('global/host-components.css'))
      .find(r => selectorList(r.selector).includes('.icon-btn.header-icon') && r.props.has('width'));
    expect(iconBox, 'no .icon-btn.header-icon rule declaring a width').toBeDefined();
    expect(iconBox!.props.get('width'), 'the header icon button is no longer square')
      .toBe(iconBox!.props.get('height'));
    expect(desktopRule('.thread-toggle-slot').props.get('width')).toBe('var(--header-icon-box)');
  });

  it('the button does not shrink with the slot, or it would squash instead of clip', () => {
    const iconBtn = cssRules(styles('global/shared-components.css'))
      .find(r => r.selector === '.icon-btn');
    expect(iconBtn?.props.get('flex-shrink')).toBe('0');
  });
});

describe('the brand region survives losing its leading child', () => {
  it('it pins its actions itself, now that nothing leads its flow', () => {
    expect(desktopRule('.app-header .pane-header-brand').props.get('justify-content'))
      .toBe('flex-end');
  });

  it('the action-collapse measurement names no leading zone here', () => {
    expect(component('layout/ThreadHeaderActions.tsx')).not.toContain('leading:');
  });
});
