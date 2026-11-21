import { hasCoarsePointer, viewportIsMobile } from './viewport';

/** A box the desktop shell holds still, and the offset it was found at. */
export interface ShellSnapBack {
  box: 'document' | 'app-shell';
  top: number;
  left: number;
}

const SHELL_SELECTOR = '.app-shell';

/** Hold the desktop shell still: neither the document nor `.app-shell` keeps a
 *  scroll offset. Returns the teardown.
 *
 *  `.app-shell` is the viewport, and every pane scrolls inside itself. An
 *  offset on either box slides the whole shell up the window, header first, and
 *  nothing in the UI can scroll it back. `overflow: hidden` stops the user
 *  scrolling them (`styles/global/base.css`), but script can still scroll a
 *  hidden box, so this snaps any offset back the moment it appears.
 *
 *  Desktop layout, fine pointer only. The phone layout leaves iOS its own
 *  keyboard scroll, and `MobileSwipeContainer` resets that one. An iPad gets
 *  the desktop layout, and iOS scrolls its document to lift a focused field
 *  above the software keyboard. A snap-back would bury the field again.
 *
 *  A scroll event fires before the frame's resize observers, so a snap-back
 *  lands before the traffic-light watch can measure a displaced header. */
export function installShellScrollGuard(onSnapBack: (snap: ShellSnapBack) => void): () => void {
  const onScroll = (e: Event): void => {
    if (viewportIsMobile.peek() || hasCoarsePointer()) return;
    const scrolled = scrolledBox(e.target);
    if (!scrolled) return;
    const { el, box } = scrolled;
    const { scrollTop: top, scrollLeft: left } = el;
    if (top === 0 && left === 0) return;
    el.scrollTop = 0;
    el.scrollLeft = 0;
    onSnapBack({ box, top, left });
  };
  document.addEventListener('scroll', onScroll, { capture: true, passive: true });
  return () => document.removeEventListener('scroll', onScroll, { capture: true });
}

function scrolledBox(target: EventTarget | null): { el: Element; box: ShellSnapBack['box'] } | null {
  if (target === document) return { el: document.scrollingElement ?? document.documentElement, box: 'document' };
  if (target instanceof Element && target.matches(SHELL_SELECTOR)) return { el: target, box: 'app-shell' };
  return null;
}
