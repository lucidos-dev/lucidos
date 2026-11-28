import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { AppsIcon, OpenInCanvasIcon, PinIcon, ReuseIcon } from '../shared/icons';
import {
  makeAppFromWidget,
  makeWidgetReusable,
  openWidgetInCanvas,
  sizeWidgetCards,
  stopReusingWidget,
} from '../../store/actions/widget-actions';
import { removeFromDashboard } from '../../store/actions/dashboards';
import { homeThreadId } from '../../store/actions/homeThread';
import type { Dashboard } from '../../generated/thread-event-wire';
import type { WidgetSize } from '../../store/types';
import { DashboardItem, Item, tileRef, type Run, type WidgetInstance } from './DashboardItem';
import { offeredSizes, WidgetSizePill } from './WidgetSizePill';

export interface WidgetMenuTarget extends WidgetInstance {
  threadId: string;
  sizes: readonly WidgetSize[];
  reusable: boolean;
  originPluginId?: string;
  builtIn: boolean;
}

export interface DashboardTileTarget extends WidgetInstance {
  dashboard: Dashboard;
}

function OpenInCanvasItem({ widget: w, run }: { widget: WidgetInstance; run: Run }) {
  return (
    <Item
      icon={<OpenInCanvasIcon />}
      label="Open in Canvas"
      hint="Full size, beside the thread"
      onClick={run(() => void openWidgetInCanvas(w.appId, w.name, w.params))}
    />
  );
}

function MakeAppItem({ widget: w, threadId, run }: { widget: WidgetInstance; threadId: string; run: Run }) {
  return (
    <Item
      icon={<AppsIcon />}
      label="Make app"
      hint="A full app in your apps list"
      onClick={run(() => makeAppFromWidget(threadId, w.appId, w.name))}
    />
  );
}

function SizeRow({ widget: w, size }: { widget: WidgetMenuTarget; size: WidgetSize }) {
  const pick = (next: WidgetSize) =>
    void sizeWidgetCards({ appId: w.appId, threadId: w.threadId, params: w.params }, w.name, next);
  return (
    <>
      <div class="widget-menu-size">
        <span>Size</span>
        <WidgetSizePill name={w.name} sizes={w.sizes} current={size} onPick={pick} />
      </div>
      <div class="thread-overflow-divider" role="separator" />
    </>
  );
}

export function widgetMenuItems({ run }: OverflowMenuContext, w: WidgetMenuTarget) {
  return (
    <>
      {w.size && offeredSizes(w.sizes).length > 0 && <SizeRow widget={w} size={w.size} />}
      <OpenInCanvasItem widget={w} run={run} />
      {!w.builtIn && !w.originPluginId && (
        <Item
          icon={<ReuseIcon />}
          label={w.reusable ? 'Stop reusing' : 'Make reusable'}
          hint={w.reusable ? 'Only this thread shows it' : 'Offer it in other threads'}
          onClick={run(() => void (w.reusable ? stopReusingWidget : makeWidgetReusable)(w.appId, w.name))}
        />
      )}
      <MakeAppItem widget={w} threadId={w.threadId} run={run} />
      <div class="thread-overflow-divider" role="separator" />
      <DashboardItem tile={w} noun="widget" run={run} />
    </>
  );
}

export function dashboardTileMenuItems({ run }: OverflowMenuContext, t: DashboardTileTarget) {
  const home = homeThreadId.value;
  return (
    <>
      <OpenInCanvasItem widget={t} run={run} />
      {home && <MakeAppItem widget={t} threadId={home} run={run} />}
      <div class="thread-overflow-divider" role="separator" />
      <Item
        icon={<PinIcon filled />}
        label="Remove from dashboard"
        onClick={run(() => void removeFromDashboard(t.dashboard, tileRef(t), t.name, 'widget'))}
      />
    </>
  );
}

export function WidgetMenuItems({ ctx, target }: { ctx: OverflowMenuContext; target: WidgetMenuTarget }) {
  return widgetMenuItems(ctx, target);
}
