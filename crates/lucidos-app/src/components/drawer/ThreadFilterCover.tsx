import type { Signal } from '@preact/signals';
import { useLayoutEffect } from 'preact/hooks';
import { threadFilterPanelOpen, setThreadFilterPaneVisible } from '../../store/threadFilterPanel';
import { drawerGrouping, type DrawerGrouping } from '../../store/store';
import { ThreadFilterPanel } from '../layout/ThreadFilterPanel';
import { NavigationCover } from '../shared/NavigationCover';
import { LeavingViewDrawing, type LeavingDrawing } from './LeavingViewDrawing';

export function drawerSwapKey(filtersOpen: boolean, grouping: DrawerGrouping): string {
  return filtersOpen ? 'filters' : `threads:${grouping}`;
}

export function ThreadFilterCover({ paneVisible, leaving }: {
  paneVisible: boolean;
  leaving: Signal<LeavingDrawing | null>;
}) {
  const open = threadFilterPanelOpen.value;
  useLayoutEffect(() => setThreadFilterPaneVisible(paneVisible), [paneVisible]);
  const swapKey = drawerSwapKey(open, drawerGrouping.value);
  return (
    <>
      <LeavingViewDrawing swapKey={swapKey} drawing={leaving} />
      <div class="thread-filter-cover" data-open={open ? '' : undefined} inert={!open || !paneVisible}>
        <ThreadFilterPanel />
      </div>
      <NavigationCover viewKey={swapKey} motion="dip" />
    </>
  );
}
