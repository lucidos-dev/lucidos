import { visibleFocusables } from '../layout/paneFocus';

/** Where a Tab out of a frame continues: its overlay panel, else the content pane. */
const EXIT_SCOPE = '[data-overlay-panel], .pane-content';

/** A focus stop placed right after an `<iframe>`. Keys pressed inside a frame
 *  never reach the host, so no trap sees a Tab past the frame's last control.
 *  The browser moves focus to the next element in DOM order, which is often
 *  outside the pane. The stop catches that Tab and hands focus on. */
export function IframeTabExit() {
  return <span class="visually-hidden" tabIndex={0} data-role="iframe-tab-exit" onFocus={onIframeTabExitFocus} />;
}

/** Forward out of the frame to the next control in scope, wrapping at the end.
 *  A Shift+Tab that reaches the stop from below goes back into the frame. A
 *  frame's own document is never the `relatedTarget`, so a focus arriving from
 *  inside it reads as null or the frame itself. Exported for unit testing. */
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
