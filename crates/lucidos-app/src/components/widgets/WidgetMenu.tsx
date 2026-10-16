import type { ComponentChildren } from 'preact';
import { useEffect } from 'preact/hooks';
import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { AppsIcon, ChevronDownIcon, OpenInCanvasIcon, PinIcon, ReuseIcon } from '../shared/icons';
import {
  makeAppFromWidget,
  makeWidgetReusable,
  openWidgetInCanvas,
  pinWidgetToShelf,
  showWidgetInThread,
  stopReusingWidget,
  unpinWidgetFromShelf,
} from '../../store/actions/widget-actions';
import { addToDashboard, removeFromDashboard, rereadDashboard } from '../../store/actions/dashboards';
import { homeThreadId, isHomeThread } from '../../store/actions/homeThread';
import { currentDashboard, dashboardFor } from '../../store/dashboards';
import { threadWidgetKey } from '../../store/widgets';
import type { Dashboard } from '../../generated/thread-event-wire';
import { widgetInstanceKey, type WidgetParams } from '../../utils/widgetParams';

/** One widget instance: what a dashboard tile names, and what a menu calls it. */
interface WidgetInstance {
  appId: string;
  params?: WidgetParams;
  /** The instance's label, which a pin or a tile records. */
  label?: string;
  /** What the menu calls it: the label, else the widget's name. */
  name: string;
}

/** The widget instance a menu acts on, in the thread it is shown in. */
export interface WidgetMenuTarget extends WidgetInstance {
  threadId: string;
  reusable: boolean;
  /** Set when a plugin ships the widget, which then stays reusable. */
  originPluginId?: string;
  /** A built-in widget ships with Lucidos and is always reusable. */
  builtIn: boolean;
  pinned: boolean;
  /** Where "Show in thread" lands. Absent on the inline widget, which is there. */
  shownEventId?: string;
  /** The event that draws this inline widget. A pin records it, so "Show in
   *  thread" lands back on it. */
  drawnByEventId?: string;
}

/** A tile on one dashboard, as its menu acts on it. */
export interface DashboardTileTarget extends WidgetInstance {
  dashboard: Dashboard;
}

type Run = OverflowMenuContext['run'];

function Item({ icon, label, hint, disabled, onClick }: {
  icon: ComponentChildren;
  label: string;
  hint?: string;
  disabled?: boolean;
  onClick: (e: MouseEvent) => void;
}) {
  return (
    <button
      type="button"
      class="thread-overflow-item widget-menu-item"
      role="menuitem"
      aria-disabled={disabled ? 'true' : undefined}
      onClick={disabled ? undefined : onClick}
    >
      {icon}
      <span class="widget-menu-label">
        <span>{label}</span>
        {hint && <span class="widget-menu-hint">{hint}</span>}
      </span>
    </button>
  );
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

function tileRef({ appId, params, label }: WidgetInstance) {
  return { appId, params, label };
}

/** The widget menu's items in a thread: the chip's right-click or long press,
 *  and the ⋯ in a widget's bar. Each item is also an agent action and a CLI
 *  verb. Home draws no shelf, so it offers no shelf pin. */
export function widgetMenuItems({ run }: OverflowMenuContext, w: WidgetMenuTarget) {
  const instance = { appId: w.appId, threadId: w.threadId, params: w.params, label: w.label };
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
      {w.shownEventId && (
        <Item
          icon={<ChevronDownIcon size="1rem" />}
          label="Show in thread"
          onClick={run(() => showWidgetInThread(w.threadId, w.shownEventId!))}
        />
      )}
      <div class="thread-overflow-divider" role="separator" />
      {!isHomeThread(w.threadId) && (w.pinned ? (
        <Item
          icon={<PinIcon filled />}
          label="Unpin from shelf"
          onClick={run(() => void unpinWidgetFromShelf(instance, w.name))}
        />
      ) : (
        <Item
          icon={<PinIcon />}
          label="Pin to shelf"
          onClick={run(() => void pinWidgetToShelf(instance, w.name, w.drawnByEventId))}
        />
      ))}
      <DashboardItem widget={w} run={run} />
    </>
  );
}

/** Add to dashboard, offered in every thread, Home included. It targets the
 *  dashboard this layout shows, and turns into Remove from dashboard once that
 *  dashboard holds the instance (ADR 0496). Until the dashboard is read, the
 *  row draws at once but stays inert, so it never offers the wrong action. */
function DashboardItem({ widget: w, run }: { widget: WidgetInstance; run: Run }) {
  const dashboard = currentDashboard();
  const view = dashboardFor(dashboard);
  useEffect(() => {
    if (view.status === 'not-loaded') void rereadDashboard(dashboard);
  }, [view.status, dashboard]);
  const key = widgetInstanceKey(w.appId, w.params);
  const held = view.status === 'loaded' && view.data.tiles.some((t) => threadWidgetKey(t) === key);
  if (held) {
    return (
      <Item
        icon={<PinIcon filled />}
        label="Remove from dashboard"
        onClick={run(() => void removeFromDashboard(dashboard, tileRef(w), w.name))}
      />
    );
  }
  return (
    <Item
      icon={<PinIcon />}
      label="Add to dashboard"
      hint={view.status === 'failed'
        ? `Couldn't read the ${dashboard} dashboard: ${view.error}`
        : `A tile on the ${dashboard} dashboard`}
      disabled={view.status !== 'loaded'}
      onClick={run(() => void addToDashboard(dashboard, tileRef(w), w.name))}
    />
  );
}

/** The widget items on a dashboard tile's menu. A tile names a reusable widget
 *  that no thread holds, so it offers no reuse change, no shelf pin and no
 *  Show in thread. Make app drafts its prompt in the Home chat. */
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
        onClick={run(() => void removeFromDashboard(t.dashboard, tileRef(t), t.name))}
      />
    </>
  );
}

/** `widgetMenuItems` as a component, for the chunk `widgetMenuChunks.ts`
 *  loads on demand. */
export function WidgetMenuItems({ ctx, target }: { ctx: OverflowMenuContext; target: WidgetMenuTarget }) {
  return widgetMenuItems(ctx, target);
}
