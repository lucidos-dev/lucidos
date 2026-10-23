import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { OverflowMenu } from '../shared/OverflowMenu';
import { useObjectMenu, type ObjectMenu } from '../shared/useObjectMenu';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';
import { OpenInCanvasIcon } from '../shared/icons';
import { AppIcon } from '../shared/AppIcon';
import { readAppsById } from '../../store/actions/appsById';
import { openWidgetInCanvas } from '../../store/actions/widget-actions';
import { appById } from '../../store/appsById';
import { threadWidget } from '../../store/widgets';
import type { App, AppReveal } from '../../store/types';
import type { WidgetParams } from '../../utils/widgetParams';
import type { WidgetMenuTarget } from './WidgetMenu';
import { WidgetMenuItems } from './widgetMenuChunks';
import { WidgetFrame, defaultWidgetHeight } from './WidgetFrame';

/** What an inline widget draws when its widget cannot: in place, never
 *  refused, since a reply cannot be (ADR 0415). */
export const WIDGET_UNAVAILABLE_TEXT = 'This widget is not available';

/** An inline widget that cannot draw, in the same box. */
export function UnavailableWidget({ appId, stamp }: { appId?: string; stamp?: string }) {
  return (
    <div class="widget-panel inline-widget inline-widget-gone" data-event-id={stamp} data-inline-widget={appId}>
      {WIDGET_UNAVAILABLE_TEXT}
    </div>
  );
}

/** A widget's bar: its app icon, its name, the word "widget", and its
 *  actions. The icon slot stays empty until the name lands. `opener` lets the
 *  panel around it open the bar's ⋯ menu from a gesture. */
function WidgetBar({ target, icon, leading, trailing, opener }: {
  target: WidgetMenuTarget;
  icon?: string;
  leading?: ComponentChildren;
  trailing?: ComponentChildren;
  opener: ObjectMenu['opener'];
}) {
  return (
    <div class="widget-bar">
      <span class="widget-bar-icon">
        {target.name && <AppIcon appId={target.appId} name={target.name} icon={icon} />}
      </span>
      <span class="widget-bar-name">{target.name}</span>
      <span class="widget-bar-kind">widget</span>
      <span class="widget-bar-actions">
        {leading}
        <button
          type="button"
          class="icon-btn header-icon"
          aria-label={`Open ${target.name} in Canvas`}
          data-tooltip="Open in Canvas"
          onClick={() => void openWidgetInCanvas(target.appId, target.name, target.params)}
        >
          <OpenInCanvasIcon />
        </button>
        <OverflowMenu
          ariaLabel={`${target.name} widget actions`}
          hostOpener={{ ref: opener, trigger: true }}
          itemsChunk={WidgetMenuItems}
          items={(ctx) => <WidgetMenuItems ctx={ctx} target={target} />}
        />
        {trailing}
      </span>
    </div>
  );
}

/** A widget's panel: its bar over its frame. An inline widget and the dropped
 *  open shelf widget draw it. A right-click or a long press anywhere on it
 *  opens the bar's ⋯ menu, inside the frame included (`frame` gets `onMenu`).
 *  The click a hold pairs is swallowed in capture, so a bar button under the
 *  finger does not also run. `stamp` and `inlineWidget` are an inline
 *  widget's deep-link stamp and app id. */
export function WidgetPanel({ class: className, stamp, inlineWidget, target, icon, leading, trailing, frame }: {
  class: string;
  stamp?: string;
  inlineWidget?: string;
  target: WidgetMenuTarget;
  icon?: string;
  leading?: ComponentChildren;
  trailing?: ComponentChildren;
  frame: (onMenu: (at?: ViewportPoint) => void) => ComponentChildren;
}) {
  const [panel, setPanel] = useState<HTMLDivElement | null>(null);
  const menu = useObjectMenu({ anchor: (el) => el.querySelector<HTMLElement>('.widget-bar') ?? el });
  const onMenu = (at?: ViewportPoint) => {
    if (panel) menu.open(panel, at);
  };
  return (
    <div
      ref={setPanel}
      class={className}
      data-event-id={stamp}
      data-inline-widget={inlineWidget}
      {...menu.press}
      onClickCapture={menu.onClick}
    >
      <WidgetBar target={target} icon={icon} leading={leading} trailing={trailing} opener={menu.opener} />
      {frame(onMenu)}
    </div>
  );
}

/** An *inline widget* (ADR 0466): a widget in a turn, with its bar over its
 *  frame. A reply draws one from its embed, and an old `WidgetShown` still
 *  draws one at its turn. `eventId` is an event of that turn: a pin from here
 *  records it, so "Show in thread" lands back on the turn.
 *
 *  `stamped` is set for an old `WidgetShown`, which is its own deep-link
 *  target (`stampedEventIds`). A reply's widget names its turn's starter,
 *  which the turn root already wears.
 *
 *  `mayDraw` is a reply embed's check, run once the widget is read. An old
 *  showing was checked when it was written, so it passes none. */
export function InlineWidget({ threadId, appId, params, label, eventId, stamped = false, mayDraw }: {
  threadId: string;
  appId: string;
  params?: WidgetParams;
  label?: string;
  eventId?: string;
  stamped?: boolean;
  mayDraw?: (app: App) => boolean;
}) {
  const stamp = stamped ? eventId : undefined;
  const read = appById(appId);
  useEffect(() => {
    if (read.status === 'not-loaded') void readAppsById([appId]);
  }, [read.status, appId]);
  const app = read.status === 'loaded' ? read.data : undefined;
  if (read.status === 'failed' || app === null || (app && (app.kind !== 'widget' || (mayDraw && !mayDraw(app))))) {
    return <UnavailableWidget appId={appId} stamp={stamp} />;
  }
  // The thread's widgets carry the pin, and refresh as a widget is renamed or
  // made reusable. The app read stands in until they name this instance.
  const shelf = threadWidget(threadId, appId, params);
  const named = shelf ?? app;
  const target: WidgetMenuTarget = {
    threadId,
    appId,
    params,
    label,
    // Blank until the widget is read: a single value, so no skeleton.
    name: label ?? shelf?.name ?? app?.name ?? '',
    reusable: shelf?.reusable ?? app?.reusable ?? false,
    originPluginId: shelf?.origin_plugin_id ?? app?.origin_plugin_id,
    builtIn: shelf?.built_in ?? app?.built_in ?? false,
    pinned: shelf?.pinned ?? false,
    drawnByEventId: eventId,
  };
  return (
    <WidgetPanel
      class="widget-panel inline-widget"
      stamp={stamp}
      inlineWidget={appId}
      target={target}
      icon={named?.icon}
      frame={(onMenu) => (app ? (
        <ClippedFrame appId={appId} params={params} name={target.name} reveal={app.reveal} onMenu={onMenu} />
      ) : (
        // The frame waits on the manifest's `reveal`. Its box holds the room.
        <div class="inline-widget-clip">
          <div class="widget-frame widget-frame-inline" style={{ height: `${defaultWidgetHeight('inline')}px` }} />
        </div>
      ))}
    />
  );
}

/** The inline frame inside a box about one phone screen tall. The frame keeps
 *  its full height, so it never scrolls inside the transcript. A widget taller
 *  than the box is clipped, and says so with a fade and an Expand button. */
function ClippedFrame({ appId, params, name, reveal, onMenu }: {
  appId: string;
  params?: WidgetParams;
  name: string;
  reveal: AppReveal;
  onMenu: (at?: ViewportPoint) => void;
}) {
  const [clip, setClip] = useState<HTMLDivElement | null>(null);
  const [height, setHeight] = useState(0);
  const [clipped, setClipped] = useState(false);

  // The frame box holds its height without a transition, so the clip box has
  // settled by the time this reads it.
  useLayoutEffect(() => {
    if (!clip) return;
    // One pixel of slack absorbs `clientHeight` rounding a fractional report.
    const measure = () => setClipped(height - clip.clientHeight > 1);
    measure();
    // The cap follows the viewport, so a resize can clip or unclip the card.
    if (typeof ResizeObserver !== 'function') return;
    const observer = new ResizeObserver(measure);
    observer.observe(clip);
    return () => observer.disconnect();
  }, [clip, height]);

  return (
    <div ref={setClip} class="inline-widget-clip">
      <WidgetFrame appId={appId} params={params} reveal={reveal} place="inline" onHeight={setHeight} onMenu={onMenu} />
      {clipped && (
        <div class="inline-widget-fade">
          <button
            type="button"
            class="action-btn action-btn-secondary"
            aria-label={`Expand ${name} in Canvas`}
            onClick={() => void openWidgetInCanvas(appId, name, params)}
          >
            Expand
          </button>
        </div>
      )}
    </div>
  );
}
