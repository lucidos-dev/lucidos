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
