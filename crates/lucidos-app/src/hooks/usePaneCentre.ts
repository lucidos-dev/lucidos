import { useLayoutEffect, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import type { FocusedPane } from '../store/store';

/** One of the two desktop panes a surface can be centred over. */
export type Pane = 'conversation' | 'canvas';

const PANE_SELECTOR: Record<Pane, string> = {
  conversation: '.split-layout > .pane-thread',
  canvas: '.split-layout > .pane-content',
};

interface PaneBox {
  centre: number;
  width: number;
}

/** A desktop pane's horizontal centre and width, in viewport px. `undefined`
 *  when that pane is not on screen: on mobile, or while it is collapsed. */
function paneBox(pane: Pane): PaneBox | undefined {
  const el = document.querySelector<HTMLElement>(PANE_SELECTOR[pane]);
  if (!el) return undefined;
  const { left, width } = el.getBoundingClientRect();
  return width > 0 ? { centre: left + width / 2, width } : undefined;
}

/** The horizontal centre of a desktop pane, in viewport px. */
export function paneCentre(pane: Pane): number | undefined {
  return paneBox(pane)?.centre;
}

/** The pane whose column holds `el`, judged by its horizontal centre. A header
 *  button is not inside a pane element, because the header spans both panes. */
export function paneUnder(el: Element | null): Pane | undefined {
  if (!el) return undefined;
  const { left, width } = el.getBoundingClientRect();
  const x = left + width / 2;
  return (Object.keys(PANE_SELECTOR) as Pane[]).find((pane) => {
    const box = document.querySelector<HTMLElement>(PANE_SELECTOR[pane])?.getBoundingClientRect();
    return box !== undefined && box.width > 0 && x >= box.left && x <= box.right;
  });
}

/** The pane a focused pane group lives in. The drawer sits beside the
 *  Conversation pane, so it counts as that one. */
export function paneOfFocus(focus: FocusedPane): Pane {
  return focus === 'content' ? 'canvas' : 'conversation';
}

/** Panel style that centres a surface over `pane` rather than the window, so
 *  it never straddles the pane divider. Pass `undefined` while closed.
 *
 *  It publishes `--pane-centre-x`, which `.surface-pane-centred` turns into a
 *  clamped shift, and `--pane-fit`, the pane's width less a 1rem margin each
 *  side. A surface that must stay inside its pane caps its width by the fit.
 *  Without either, the panel stays centred on the window, which is right on
 *  mobile.
 *
 *  It re-measures when either pane changes size, as well as on window resize.
 *  A divider drag or a maximize moves the pane without resizing the window. */
export function usePaneCentre(pane: Pane | undefined): JSX.CSSProperties | undefined {
  const [box, setBox] = useState(() => (pane ? paneBox(pane) : undefined));
  useLayoutEffect(() => {
    // Keeps the old box when the measurement is unchanged, so a resize that
    // moves nothing re-renders nothing.
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
