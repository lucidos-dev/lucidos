import { describe, it, expect, beforeEach } from 'vitest';
import { drawerOpen } from '../Drawer';
import { threadDrawerOpen, mobileView } from '../../../store/store';
import { MobileDotIndicator } from '../MobileAppHeader';
import { navigateToPane } from '../../../store/actions/pane';

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
