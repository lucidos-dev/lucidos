import { useEffect, useRef } from 'preact/hooks';
import { clientRefreshing } from '../../hooks/sw-update';

export function UiBlockingOverlay() {
  const active = clientRefreshing.value;
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!active) return;
    const overlay = overlayRef.current;
    if (!overlay) return;

    const block = (e: Event) => {
      e.stopPropagation();
      e.preventDefault();
    };

    document.addEventListener('keydown', block, true);
    document.addEventListener('keyup', block, true);

    (document.activeElement as HTMLElement | null)?.blur();

    document.documentElement.setAttribute('data-ui-blocked', '');

    const inerted: HTMLElement[] = [];
    const inertChildrenOf = (parent: Element) => {
      for (const child of Array.from(parent.children) as HTMLElement[]) {
        if (child === overlay) continue;
        if (child.matches('[data-overlay-layer-home], .overlay-layer')) {
          inertChildrenOf(child);
          continue;
        }
        if (child.classList.contains('toast-container')) continue;
        if (child.hasAttribute('inert')) continue;
        child.setAttribute('inert', '');
        inerted.push(child);
      }
    };
    if (overlay.parentElement) inertChildrenOf(overlay.parentElement);


    return () => {
      document.removeEventListener('keydown', block, true);
      document.removeEventListener('keyup', block, true);
      inerted.forEach((el) => el.removeAttribute('inert'));
      document.documentElement.removeAttribute('data-ui-blocked');
    };
  }, [active]);

  if (!active) return null;

  return <div class="ui-blocking-overlay" ref={overlayRef} />;
}
