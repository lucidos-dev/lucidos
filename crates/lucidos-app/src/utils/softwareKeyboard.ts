import { hasCoarsePointer } from './viewport';

const PROXY_ATTR = 'data-keyboard-proxy';

export function isOverlayField(el: Element): boolean {
  return el.hasAttribute(PROXY_ATTR) || el.closest('[data-overlay-panel]') !== null;
}

export function holdSoftwareKeyboard(): void {
  if (!hasCoarsePointer()) return;
  const proxy = document.createElement('input');
  proxy.setAttribute(PROXY_ATTR, '');
  proxy.style.cssText = 'position:fixed;top:-9999px;left:0;opacity:0;width:1px;height:1px;';
  document.body.appendChild(proxy);
  proxy.focus({ preventScroll: true });
  setTimeout(() => proxy.remove(), 500);
}
