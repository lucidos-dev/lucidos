import { hasCoarsePointer, viewportIsMobile } from './viewport';

export interface ShellSnapBack {
  box: 'document' | 'app-shell';
  top: number;
  left: number;
}

const SHELL_SELECTOR = '.app-shell';

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
