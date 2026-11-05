import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { AppsIcon, OpenInCanvasIcon, PinIcon, ReuseIcon } from '../shared/icons';
import {
  makeAppFromWidget,
  makeWidgetReusable,
  openWidgetInCanvas,
  stopReusingWidget,
} from '../../store/actions/widget-actions';
import { removeFromDashboard } from '../../store/actions/dashboards';
import { homeThreadId } from '../../store/actions/homeThread';
import type { Dashboard } from '../../generated/thread-event-wire';
import { DashboardItem, Item, tileRef, type Run, type WidgetInstance } from './DashboardItem';

/** The widget instance a menu acts on, in the thread it is shown in. */
export interface WidgetMenuTarget extends WidgetInstance {
  threadId: string;
  reusable: boolean;
  /** Set when a plugin ships the widget, which then stays reusable. */
  originPluginId?: string;
  /** A built-in widget ships with Lucidos and is always reusable. */
  builtIn: boolean;
}

/** A tile on one dashboard, as its menu acts on it. */
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

/** The widget menu's items in a thread: the ⋯ in a widget's bar, or a
 *  right-click or long press on its card. Each item is also an agent action
 *  and a CLI verb. */
export function widgetMenuItems({ run }: OverflowMenuContext, w: WidgetMenuTarget) {
  return (
    <>
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

/** The widget items on a dashboard tile's menu. A tile names a reusable widget
 *  that no thread holds, so it offers no reuse change. Make app drafts its
 *  prompt in the Home chat. */
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

/** `widgetMenuItems` as a component, for the chunk `widgetMenuChunks.ts`
 *  loads on demand. */
export function WidgetMenuItems({ ctx, target }: { ctx: OverflowMenuContext; target: WidgetMenuTarget }) {
  return widgetMenuItems(ctx, target);
}
