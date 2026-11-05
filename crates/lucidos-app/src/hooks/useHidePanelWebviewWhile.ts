import { useEffect } from 'preact/hooks';
import { isTauri } from '../utils/platform';
import { hidePanelWebview, showPanelWebview } from '../utils/tauri';

/** Pure orchestration for {@link useHidePanelWebviewWhile}: while `active`, hold
 *  the panel webview hidden by calling `hide()`, and return a cleanup that calls
 *  `show()` to release it. When `active` is false it does nothing (and returns
 *  `undefined`), so the webview stays visible. Exported hook-free so the
 *  hide/show pairing is unit-testable without a DOM or Tauri — same style as
 *  `delayedFlagEffect` in useDelayedLoading.ts. The ref-counting that lets
 *  several overlays request hide concurrently lives in `utils/tauri.ts`. */
export function panelWebviewVisibilityEffect(
  active: boolean,
  hide: () => void,
  show: () => void,
): (() => void) | undefined {
  if (!active) return undefined;
  hide();
  return show;
}

/** Hide the Tauri panel webview (the internal browser's native overlay) while
 *  `active` is true, restoring it when `active` flips false or the caller
 *  unmounts. The native webview paints ABOVE all HTML, so any overlay that can
 *  render over the content pane — dropdowns, the drawer, modal dialogs, the
 *  back/forward nav-history popover — must hide it or it renders behind the
 *  browser and the user can't see or click it. No-op outside Tauri. */
export function useHidePanelWebviewWhile(active: boolean): void {
  useEffect(() => {
    if (!isTauri()) return;
    return panelWebviewVisibilityEffect(active, hidePanelWebview, showPanelWebview);
  }, [active]);
}
