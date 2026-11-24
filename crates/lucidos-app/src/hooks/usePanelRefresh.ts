import { useEffect, useRef } from 'preact/hooks';
import { registerPanelRefresh, type PanelRefreshAction } from '../store/panelRefresh';

export function usePanelRefresh(noun: string, action: PanelRefreshAction | null): void {
  const latest = useRef(action);
  latest.current = action;
  const enabled = action !== null;
  useEffect(() => {
    if (!enabled) return;
    return registerPanelRefresh(noun, () => latest.current?.() ?? Promise.resolve());
  }, [noun, enabled]);
}
