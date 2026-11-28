import { visibleFocusables } from '../layout/paneFocus';

const EXIT_SCOPE = '[data-overlay-panel], .pane-content';

export function IframeTabExit() {
  return <span class="visually-hidden" tabIndex={0} data-role="iframe-tab-exit" onFocus={onIframeTabExitFocus} />;
}

export function onIframeTabExitFocus(e: Pick<FocusEvent, 'currentTarget' | 'relatedTarget'>): void {
  const stop = e.currentTarget as HTMLElement;
  const frame = stop.previousElementSibling as HTMLElement | null;
  const from = e.relatedTarget as Node | null;
  if (from && from !== frame) {
    frame?.focus({ preventScroll: true });
    return;
  }
  const scope = stop.closest<HTMLElement>(EXIT_SCOPE);
  if (!scope) return;
  const rest = visibleFocusables(scope).filter((el) => el !== stop);
  const next = rest.find((el) => stop.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING) ?? rest[0];
  next?.focus({ preventScroll: true });
}
