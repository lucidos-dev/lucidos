import type { ComponentChildren } from 'preact';
import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { AppsIcon, ChevronDownIcon, OpenInCanvasIcon, PinIcon, ReuseIcon } from '../shared/icons';
import {
  makeAppFromWidget,
  makeWidgetReusable,
  openWidgetInCanvas,
  pinWidgetToHome,
  pinWidgetToShelf,
  showWidgetInThread,
  stopReusingWidget,
  unpinWidgetFromShelf,
} from '../../store/actions/widget-actions';
import { homeThreadId, isHomeThread } from '../../store/actions/homeThread';
import { threadWidget } from '../../store/widgets';
import type { WidgetParams } from '../../utils/widgetParams';

/** The widget instance a menu acts on, in the thread it is shown in. */
export interface WidgetMenuTarget {
  threadId: string;
  appId: string;
  params?: WidgetParams;
  /** The instance's label, which the pin records for its chip. */
  label?: string;
  /** What the menu calls it: the label, else the widget's name. */
  name: string;
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

function Item({ icon, label, hint, onClick }: {
  icon: ComponentChildren;
  label: string;
  hint?: string;
  onClick: (e: MouseEvent) => void;
}) {
  return (
    <button type="button" class="thread-overflow-item widget-menu-item" role="menuitem" onClick={onClick}>
      {icon}
      <span class="widget-menu-label">
        <span>{label}</span>
        {hint && <span class="widget-menu-hint">{hint}</span>}
      </span>
    </button>
  );
}

/** The widget menu's items: the chip's right-click or long press, and the ⋯
 *  in a widget's bar. Each item is also an agent action and a CLI verb. */
export function widgetMenuItems({ run }: OverflowMenuContext, w: WidgetMenuTarget) {
  const instance = { appId: w.appId, threadId: w.threadId, params: w.params, label: w.label };
  return (
    <>
      <Item
        icon={<OpenInCanvasIcon />}
        label="Open in Canvas"
        hint="Full size, beside the thread"
        onClick={run(() => void openWidgetInCanvas(w.appId, w.name, w.params))}
      />
      {!w.builtIn && !w.originPluginId && (
        <Item
          icon={<ReuseIcon />}
          label={w.reusable ? 'Stop reusing' : 'Make reusable'}
          hint={w.reusable ? 'Only this thread shows it' : 'Offer it in other threads'}
          onClick={run(() => void (w.reusable ? stopReusingWidget : makeWidgetReusable)(w.appId, w.name))}
        />
      )}
      <Item
        icon={<AppsIcon />}
        label="Make app"
        hint="A full app in your apps list"
        onClick={run(() => makeAppFromWidget(w.threadId, w.appId, w.name))}
      />
      {w.shownEventId && (
        <Item
          icon={<ChevronDownIcon size="1rem" />}
          label="Show in thread"
          onClick={run(() => showWidgetInThread(w.threadId, w.shownEventId!))}
        />
      )}
      <div class="thread-overflow-divider" role="separator" />
      {w.pinned ? (
        <Item
          icon={<PinIcon filled />}
          label={isHomeThread(w.threadId) ? 'Unpin from Home' : 'Unpin from shelf'}
          onClick={run(() => void unpinWidgetFromShelf(instance, w.name))}
        />
      ) : (
        <PinItem threadId={w.threadId} onClick={run(() => void pinWidgetToShelf(instance, w.name, w.drawnByEventId))} />
      )}
      <PinToHomeItem target={w} run={run} />
    </>
  );
}

/** Every thread but Home offers Pin to Home beside Pin to shelf, until the
 *  instance is on Home's board (ADR 0432). */
function PinToHomeItem({ target: w, run }: { target: WidgetMenuTarget; run: OverflowMenuContext['run'] }) {
  const home = homeThreadId.value;
  if (!home || home === w.threadId || threadWidget(home, w.appId, w.params)?.pinned) return null;
  return (
    <Item
      icon={<PinIcon />}
      label="Pin to Home"
      hint="A tile on the Home board"
      onClick={run(() => void pinWidgetToHome({ appId: w.appId, params: w.params, label: w.label }, w.name))}
    />
  );
}

/** Home draws no shelf, so a pin there names Home. */
function PinItem({ threadId, onClick }: { threadId: string; onClick: (e: MouseEvent) => void }) {
  return <Item icon={<PinIcon />} label={isHomeThread(threadId) ? 'Pin to Home' : 'Pin to shelf'} onClick={onClick} />;
}

/** `widgetMenuItems` as a component, for the chunk `widgetMenuChunks.ts`
 *  loads on demand. */
export function WidgetMenuItems({ ctx, target }: { ctx: OverflowMenuContext; target: WidgetMenuTarget }) {
  return widgetMenuItems(ctx, target);
}
