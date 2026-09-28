import { useEffect, useRef } from 'preact/hooks';
import { registerPanelRefresh, type PanelRefreshAction } from '../store/panelRefresh';

/** Register this panel's refresh while it is mounted (the panel refresh
 *  contract, `store/panelRefresh.ts`). Pass `null` while there is nothing to
 *  refresh, such as a file open in the editor.
 *
 *  The action is read through a ref, so a caller may pass a fresh closure each
 *  render without re-registering. */
export function usePanelRefresh(noun: string, action: PanelRefreshAction | null): void {
  const latest = useRef(action);
  latest.current = action;
  const enabled = action !== null;
  useEffect(() => {
    if (!enabled) return;
    return registerPanelRefresh(noun, () => latest.current?.() ?? Promise.resolve());
  }, [noun, enabled]);
}
