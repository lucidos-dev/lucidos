/**
 * The thread-drawer toggle draws what it opens: a sidebar on desktop, a list on mobile.
 *
 * On desktop the header already carries a stack of horizontal lines, the Canvas
 * pane's hamburger. A toggle drawn the same way reads as a second, which is half
 * of why a tester could not find it. On a phone the toggle opens a whole pane of
 * threads, so a sidebar glyph there lies about what it does.
 *
 * Filter draws a funnel, never lines. On a phone it holds the same corner of the
 * threads pane that the list toggle holds on the pane beside it.
 *
 * Invokes the component directly and walks the returned VNode tree, the same way
 * `thread-toggle-attention-badge.test.tsx` does: the button is hook-free at its
 * own level, and the walk never calls a nested component.
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { ThreadToggleButton } from '../ThreadToggleButton';
import { FilteredIcon, FilterIcon, SidebarIcon, ThreadListIcon } from '../icons';
import { mobileView, threadDrawerOpen, threadMap } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';
import { componentTypes, findByType, type AnyVNode } from '../../layout/__tests__/vnodeWalk';

beforeEach(() => {
  threadMap.value = new Map();
  threadDrawerOpen.value = false;
  mobileView.value = 'thread';
  viewportIsMobile.value = false;
});

describe('the thread-drawer toggle glyph', () => {
  it('is the sidebar icon on desktop', () => {
    const types = componentTypes(ThreadToggleButton({}) as AnyVNode);
    expect(types).toContain(SidebarIcon);
    expect(types).not.toContain(ThreadListIcon);
  });

  it('is the thread list icon on mobile, where it opens a whole pane', () => {
    viewportIsMobile.value = true;
    const types = componentTypes(ThreadToggleButton({}) as AnyVNode);
    expect(types).toContain(ThreadListIcon);
    expect(types).not.toContain(SidebarIcon);
  });

  it('draws a framed window on desktop, not a stack of horizontal lines', () => {
    const d = String(findByType(SidebarIcon() as AnyVNode, 'path')[0]?.props.d);
    expect(d, 'no rounded window frame').toMatch(/a2 2/);
    const horizontal = d.split('M').filter(sub => /^[\d.\s]+h[\d.\s-]+$/.test(sub.trim()));
    expect(horizontal, 'a horizontal line makes it read as the hamburger').toHaveLength(0);
  });

  it('paints the frame and its divider as ONE shape', () => {
    // Two elements stroke their crossing twice, and a translucent colour then
    // shows a bright dot at each end of the divider.
    const glyph = SidebarIcon() as AnyVNode;
    expect(findByType(glyph, 'path')).toHaveLength(1);
    for (const tag of ['rect', 'line', 'polyline', 'circle']) {
      expect(findByType(glyph, tag), tag).toHaveLength(0);
    }
  });

  it('draws bullets on mobile', () => {
    const glyph = ThreadListIcon() as AnyVNode;
    expect(findByType(glyph, 'circle'), 'one bullet per row').toHaveLength(3);
    expect(findByType(glyph, 'line')).toHaveLength(3);
  });

  it('never shares its lines with the Filter glyph, which takes the same corner on the next pane', () => {
    for (const Icon of [FilterIcon, FilteredIcon]) {
      expect(findByType(Icon() as AnyVNode, 'line'), Icon.name).toHaveLength(0);
    }
  });
});
