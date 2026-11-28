import { useEffect } from 'preact/hooks';
import { isTauri } from '../utils/platform';
import { hidePanelWebview, showPanelWebview } from '../utils/tauri';

export function panelWebviewVisibilityEffect(
  active: boolean,
  hide: () => void,
  show: () => void,
): (() => void) | undefined {
  if (!active) return undefined;
  hide();
  return show;
}

export function useHidePanelWebviewWhile(active: boolean): void {
  useEffect(() => {
    if (!isTauri()) return;
    return panelWebviewVisibilityEffect(active, hidePanelWebview, showPanelWebview);
  }, [active]);
}
