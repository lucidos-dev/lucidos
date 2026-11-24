import { useLayoutEffect, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import type { FocusedPane } from '../store/store';

export type Pane = 'conversation' | 'canvas';

const PANE_SELECTOR: Record<Pane, string> = {
  conversation: '.split-layout > .pane-thread',
  canvas: '.split-layout > .pane-content',
};

interface PaneBox {
  centre: number;
  width: number;
}

function paneBox(pane: Pane): PaneBox | undefined {
  const el = document.querySelector<HTMLElement>(PANE_SELECTOR[pane]);
  if (!el) return undefined;
  const { left, width } = el.getBoundingClientRect();
  return width > 0 ? { centre: left + width / 2, width } : undefined;
}

export function paneCentre(pane: Pane): number | undefined {
  return paneBox(pane)?.centre;
}

export function paneUnder(el: Element | null): Pane | undefined {
  if (!el) return undefined;
  const { left, width } = el.getBoundingClientRect();
  const x = left + width / 2;
  return (Object.keys(PANE_SELECTOR) as Pane[]).find((pane) => {
    const box = document.querySelector<HTMLElement>(PANE_SELECTOR[pane])?.getBoundingClientRect();
    return box !== undefined && box.width > 0 && x >= box.left && x <= box.right;
  });
}

export function paneOfFocus(focus: FocusedPane): Pane {
  return focus === 'content' ? 'canvas' : 'conversation';
}

export function usePaneCentre(pane: Pane | undefined): JSX.CSSProperties | undefined {
  const [box, setBox] = useState(() => (pane ? paneBox(pane) : undefined));
  useLayoutEffect(() => {
    const sync = () => setBox((prev) => {
      const next = pane ? paneBox(pane) : undefined;
      return prev && next && prev.centre === next.centre && prev.width === next.width ? prev : next;
    });
    sync();
    if (!pane) return;
    window.addEventListener('resize', sync);
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(sync);
    for (const selector of Object.values(PANE_SELECTOR)) {
      const el = document.querySelector(selector);
      if (el) observer?.observe(el);
    }
    return () => {
      window.removeEventListener('resize', sync);
      observer?.disconnect();
    };
  }, [pane]);
  return box === undefined
    ? undefined
    : { '--pane-centre-x': `${box.centre}px`, '--pane-fit': `calc(${box.width}px - 2rem)` };
}
