import type { ComponentChildren } from 'preact';
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

/** The widget a menu acts on, in the thread it is shown in. */
export interface WidgetMenuTarget {
  threadId: string;
  appId: string;
  name: string;
  reusable: boolean;
  pinned: boolean;
  /** Where "Show in thread" lands. Absent on the thread's card, which is there. */
  shownEventId?: string;
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
  return (
    <>
      <Item
        icon={<OpenInCanvasIcon />}
        label="Open in Canvas"
        hint="Full size, beside the thread"
        onClick={run(() => void openWidgetInCanvas(w.appId, w.name))}
      />
      <Item
        icon={<ReuseIcon />}
        label={w.reusable ? 'Stop reusing' : 'Make reusable'}
        hint={w.reusable ? 'Only this thread shows it' : 'Offer it in other threads'}
        onClick={run(() => void (w.reusable ? stopReusingWidget : makeWidgetReusable)(w.appId, w.name))}
      />
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
          label="Unpin from shelf"
          onClick={run(() => void unpinWidgetFromShelf(w.appId, w.name, w.threadId))}
        />
      ) : (
        <Item
          icon={<PinIcon />}
          label="Pin to shelf"
          onClick={run(() => void pinWidgetToShelf(w.appId, w.name, w.threadId))}
        />
      )}
    </>
  );
}
