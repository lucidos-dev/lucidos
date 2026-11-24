export function isElementVisible(el: HTMLElement): boolean {
  return unmeasurableBox(el) === null;
}

export function unmeasurableBox(el: HTMLElement): HTMLElement | null {
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return el;
  let ancestor = el.parentElement;
  while (ancestor && ancestor !== document.documentElement) {
    if (getComputedStyle(ancestor).display === 'contents') {
      ancestor = ancestor.parentElement;
      continue;
    }
    const ar = ancestor.getBoundingClientRect();
    if (ar.height <= 0 || ar.width <= 0) return ancestor;
    ancestor = ancestor.parentElement;
  }
  return null;
}
