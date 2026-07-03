// IIFE bundle entry — adds iframe-only side effects to keep `index.ts` ES-import-safe for the frontend.
import { lucidos } from './index';
import { installScrollMemory } from './scroll';
import { installKeyboardForwarding } from './keyboardForward';

export * from './index';

if (typeof document !== 'undefined') {
  installScrollMemory();
  // Forward host keyboard shortcuts (pane focus/hide, narrow/widen, Escape, …)
  // up to the parent — iframe keydowns never reach the host document otherwise,
  // so shortcuts die whenever an app has focus. See keyboardForward.ts.
  installKeyboardForwarding();

  document.addEventListener('click', (e: MouseEvent) => {
    const target = e.target as Element | null;
    const anchor = target?.closest?.('a[href]') as HTMLAnchorElement | null;
    if (!anchor) return;
    const href = anchor.getAttribute('href');
    if (!href) return;
    if (/^https?:\/\//.test(href)) {
      e.preventDefault();
      e.stopPropagation();
      lucidos.ui.navigate('url', { url: href }).catch((err) => {
        console.warn('[lucidos-sdk] navigate fell back to window.open:', err);
        window.open(href, '_blank');
      });
    } else if (anchor.getAttribute('target') === '_blank') {
      e.preventDefault();
      e.stopPropagation();
      const resolved = new URL(href, window.location.href).pathname;
      window.location.href = resolved;
    }
  }, true);
}
