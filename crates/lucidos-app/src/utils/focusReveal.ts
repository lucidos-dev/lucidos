
export interface RevealGeometry {
  scrollTop: number;
  viewTop: number;
  viewBottom: number;
  fieldTop: number;
  fieldBottom: number;
  marginTop: number;
  marginBottom: number;
}

const MIN_MOVE_PX = 1;

export function revealScrollTop(g: RevealGeometry): number | null {
  const { scrollTop, viewTop, viewBottom, fieldTop, fieldBottom, marginTop, marginBottom } = g;
  const bottomFit = scrollTop + fieldBottom + marginBottom - viewBottom;
  const topFit = scrollTop + fieldTop - marginTop - viewTop;
  const fit = bottomFit > topFit
    ? topFit
    : Math.min(Math.max(scrollTop, bottomFit), topFit);
  const target = Math.max(0, fit);
  if (Math.abs(target - scrollTop) < MIN_MOVE_PX) return null;
  return target;
}

export function nearestScrollableAncestor(el: Element | null): HTMLElement | null {
  for (let node = el?.parentElement ?? null; node; node = node.parentElement) {
    if (node.scrollHeight - node.clientHeight <= MIN_MOVE_PX) continue;
    const { overflowY } = getComputedStyle(node);
    if (overflowY === 'auto' || overflowY === 'scroll') return node;
  }
  return null;
}
