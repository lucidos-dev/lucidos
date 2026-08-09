import { describe, it, expect, beforeEach } from 'vitest';
// @ts-expect-error - Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error - same
import { dirname, resolve } from 'node:path';
// @ts-expect-error - same
import { fileURLToPath } from 'node:url';
import { drawerOpen } from '../Drawer';
import { threadDrawerOpen, mobileView } from '../../../store/store';
import { MobileDotIndicator } from '../MobileAppHeader';
import { navigateToPane } from '../../../store/actions/pane';
import { cssRules } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const mobileCss = readFileSync(
  resolve(here, '../../../styles/mobile.css'),
  'utf-8',
);

function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const m = mobileCss.match(new RegExp(`(^|\\n)\\s*${escaped}\\s*\\{([^}]*)\\}`));
  if (!m) throw new Error(`${selector} rule not found`);
  return m[2];
}

describe('MobileDotIndicator — always visible', () => {
  beforeEach(() => {
    drawerOpen.value = false;
    threadDrawerOpen.value = false;
    mobileView.value = 'thread';
  });

  it('renders dots when no drawers are open', () => {
    const vnode = (MobileDotIndicator as () => unknown)();
    expect(vnode).not.toBeNull();
  });

  it('renders dots when hamburger drawer is open', () => {
    drawerOpen.value = true;
    const vnode = (MobileDotIndicator as () => unknown)();
    expect(vnode).not.toBeNull();
  });

  it('renders dots when thread drawer is open', () => {
    threadDrawerOpen.value = true;
    const vnode = (MobileDotIndicator as () => unknown)();
    expect(vnode).not.toBeNull();
  });

  it('renders dots when both drawers are open', () => {
    drawerOpen.value = true;
    threadDrawerOpen.value = true;
    const vnode = (MobileDotIndicator as () => unknown)();
    expect(vnode).not.toBeNull();
  });
});

describe('MobileDotIndicator — header contrast', () => {
  it('uses the header foreground token for inactive dots and the active-pane tokens for the active dot', () => {
    expect(ruleBody('.mobile-dot')).toMatch(/background:\s*var\(--header-fg-muted,\s*var\(--text-muted\)\)/);
    // Active dot wears the per-theme active-pane fill + glow (base.css
    // --focus-pill-*).
    expect(ruleBody('.mobile-dot.active')).toMatch(/background:\s*var\(--focus-pill-bg\)/);
    expect(ruleBody('.mobile-dot.active')).toMatch(/box-shadow:\s*var\(--focus-pill-glow\)/);
  });
});

describe('MobileDotIndicator: centred under the row, and still tappable', () => {
  // NOT `ruleBody`: `.mobile-dot-indicator` has a base `display: none` rule
  // outside the media query, and a first-textual-match reads that one.
  const rule = cssRules(mobileCss).find(
    r => r.selector === '.mobile-dot-indicator' && r.atRules === '@media (max-width: 768px)',
  );
  const band = () => {
    expect(rule, 'the mobile .mobile-dot-indicator rule').toBeDefined();
    return rule!;
  };

  it('derives its pull from the row height and the glyph run, not a constant', () => {
    // The row is as tall as the Lucidos mark's TAP TARGET, which is a
    // style-remote tunable and much larger than any glyph in the row. A
    // hand-tuned `-0.25rem` was balanced only at the mark's shipped size:
    // retuned to 2.6rem it left 13.5px above the dots against 7.5px below.
    const margin = band().props.get('margin-top')?.replace(/\s+/g, ' ');
    expect(margin).toBe(
      'calc( -1 * var(--pane-header-gap) - '
      + '(var(--mobile-header-row-height) - var(--mobile-header-glyph-box)) / 2 )',
    );
  });

  it('keeps the two gaps equal, which is what "centred" means here', () => {
    // One token on both sides. A padding-bottom without a matching top is how
    // the band went lopsided in the first place.
    expect(band().props.get('padding')).toBe('var(--mobile-dot-gap) 0');
    expect(band().props.get('padding-bottom')).toBeUndefined();
  });

  it('paints and hit-tests above the row it now overlaps', () => {
    // The pull puts the band under the row's bottom edge, where the centred nav
    // cluster lives. That cluster is absolutely positioned, so without its own
    // stacking context the band loses the middle dot's tap to it.
    expect(band().props.get('position')).toBe('relative');
    expect(band().props.get('z-index')).toBe('1');
  });

  it('lets everything BUT the dots through, so it cannot eat a button', () => {
    // The band is full-width and empty, and raised over the row it now covers
    // the bottom few pixels of every icon button in it. A transparent div is a
    // hit target all the same: measured, a tap on the drawer toggle's bottom
    // edge landed on the band and did nothing. Only the dots are targets.
    expect(band().props.get('pointer-events')).toBe('none');
    const dot = cssRules(mobileCss).find(
      r => r.selector === '.mobile-dot' && r.atRules === '@media (max-width: 768px)',
    );
    expect(dot?.props.get('pointer-events')).toBe('auto');
  });

  it('still goes inert with the keyboard up, dots included', () => {
    // The dots re-enable their own pointer events above, so the keyboard-active
    // rule can no longer reach them through the band alone.
    const inert = cssRules(mobileCss).filter(
      r => r.selector.includes('[data-keyboard-active]')
        && r.props.get('pointer-events') === 'none'
        && /\.mobile-dot/.test(r.selector),
    );
    expect(inert).toHaveLength(1);
    expect(inert[0].selector).toContain(':root[data-keyboard-active] .mobile-dot-indicator');
    expect(inert[0].selector).toContain(':root[data-keyboard-active] .mobile-dot');
  });
});

describe('MobileDotIndicator — closes drawers on tap', () => {
  beforeEach(() => {
    drawerOpen.value = false;
    threadDrawerOpen.value = false;
    mobileView.value = 'content';
  });

  it('closes hamburger drawer when dot is tapped', () => {
    drawerOpen.value = true;
    navigateToPane('threads');
    expect(drawerOpen.value).toBe(false);
  });

  it('closes thread drawer when dot is tapped', () => {
    mobileView.value = 'thread';
    threadDrawerOpen.value = true;
    navigateToPane('content');
    expect(threadDrawerOpen.value).toBe(false);
  });

  it('closes both drawers when dot is tapped', () => {
    mobileView.value = 'thread';
    drawerOpen.value = true;
    threadDrawerOpen.value = true;
    navigateToPane('threads');
    expect(drawerOpen.value).toBe(false);
    expect(threadDrawerOpen.value).toBe(false);
  });
});
