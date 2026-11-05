import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { OverflowMenu } from '../shared/OverflowMenu';
import { useObjectMenu, type ObjectMenu } from '../shared/useObjectMenu';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';
import { AppIcon } from '../shared/AppIcon';
import { readAppsById } from '../../store/actions/appsById';
import { appById } from '../../store/appsById';
import { widgetCardSize } from '../../store/widgetCardSizes';
import { sizeWidgetCards } from '../../store/actions/widget-actions';
import type { App, WidgetSize } from '../../store/types';
import type { WidgetParams } from '../../utils/widgetParams';
import { widgetRefusal } from '../../utils/widgetEmbed';
import { widgetBoxCss } from '../../utils/dashboardGeometry';
import { AddToDashboardButton } from './DashboardItem';
import type { WidgetMenuTarget } from './WidgetMenu';
import { WidgetMenuItems } from './widgetMenuChunks';
import { WidgetBox } from './WidgetBox';
import { WidgetSizePill } from './WidgetSizePill';

/** What an inline widget draws when its widget cannot: in place, never
 *  refused, since a reply cannot be (ADR 0415). */
export const WIDGET_UNAVAILABLE_TEXT = 'This widget is not available';

/** `reason` as a sentence: the embed parser words its reasons as clauses. */
function asSentence(reason: string): string {
  const capped = reason.charAt(0).toUpperCase() + reason.slice(1);
  return /[.!?]$/.test(capped) ? capped : `${capped}.`;
}

/** An inline widget that cannot draw, in the same box. `reason` says why, so
 *  whoever wrote the embed can fix it. */
export function UnavailableWidget({ appId, stamp, reason }: { appId?: string; stamp?: string; reason?: string }) {
  return (
    <div class="widget-panel inline-widget inline-widget-gone" data-event-id={stamp} data-inline-widget={appId}>
      {WIDGET_UNAVAILABLE_TEXT}
      {reason && <div class="inline-widget-gone-reason">{asSentence(reason)}</div>}
    </div>
  );
}

/** A widget's bar: its app icon, its name, and its actions, the ⋯ last. A
 *  `compact` bar, a small card's, folds its name into the icon's tooltip. The
 *  icon slot stays empty until the name lands. `opener` lets the panel around
 *  it open the bar's ⋯ menu from a gesture. */
function WidgetBar({ target, icon, compact = false, leading, trailing, opener }: {
  target: WidgetMenuTarget;
  icon?: string;
  compact?: boolean;
  leading?: ComponentChildren;
  trailing?: ComponentChildren;
  opener: ObjectMenu['opener'];
}) {
  return (
    <div class="widget-bar">
      <span class="widget-bar-icon" data-tooltip={compact ? target.name || undefined : undefined}>
        {target.name && <AppIcon appId={target.appId} name={target.name} icon={icon} />}
      </span>
      {!compact && <span class="widget-bar-name">{target.name}</span>}
      <span class="widget-bar-actions">
        {leading}
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

/** A widget's panel: its bar over its frame, as an inline widget draws it.
 *  A right-click or a long press anywhere on it opens the bar's ⋯ menu,
 *  inside the frame included (`frame` gets `onMenu`). The click a hold pairs
 *  is swallowed in capture, so a bar button under the finger does not also
 *  run. `stamp` and `inlineWidget` are an inline
 *  widget's deep-link stamp and app id, and `style` its box geometry. */
export function WidgetPanel({ class: className, style, stamp, inlineWidget, target, icon, compact, leading, trailing, frame }: {
  class: string;
  style?: Record<string, string>;
  stamp?: string;
  inlineWidget?: string;
  target: WidgetMenuTarget;
  icon?: string;
  compact?: boolean;
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
      style={style}
      data-event-id={stamp}
      data-inline-widget={inlineWidget}
      {...menu.press}
      onClickCapture={menu.onClick}
    >
      <WidgetBar target={target} icon={icon} compact={compact} leading={leading} trailing={trailing} opener={menu.opener} />
      {frame(onMenu)}
    </div>
  );
}

/** The size a card holds its room at until its widget is read. */
const UNREAD_BOX_SIZE: WidgetSize = 'medium';

/** An *inline widget* (ADR 0466): a widget in a turn, its bar over its
 *  *widget box*, the same box its dashboard tile draws (ADR 0432). A reply
 *  draws one from its embed, and an old `WidgetShown` still draws one at its
 *  turn. `eventId` is an event of that turn.
 *
 *  The card starts at the widget's default size, and its pill picks another
 *  of the sizes it declares. A pick writes `InlineWidgetSized`, and the card
 *  draws the newest one the thread holds. It resizes the box and keeps the
 *  frame.
 *
 *  `stamped` is set for an old `WidgetShown`, which is its own deep-link
 *  target (`stampedEventIds`). A reply's widget names its turn's starter,
 *  which the turn root already wears.
 *
 *  `refusal` is a reply embed's check, run once the widget is read. An old
 *  showing was checked when it was written, so it passes none. */
export function InlineWidget({ threadId, appId, params, label, eventId, stamped = false, refusal = widgetRefusal }: {
  threadId: string;
  appId: string;
  params?: WidgetParams;
  label?: string;
  eventId?: string;
  stamped?: boolean;
  refusal?: (app: App) => string | null;
}) {
  const stamp = stamped ? eventId : undefined;
  const read = appById(appId);
  useEffect(() => {
    if (read.status === 'not-loaded') void readAppsById([appId]);
  }, [read.status, appId]);
  const app = read.status === 'loaded' ? read.data : undefined;
  const sizes = app?.sizes ?? [];
  const reason = read.status === 'failed'
    ? read.error
    : app === null
      ? `No widget '${appId}' exists.`
      : app ? refusal(app) ?? (sizes.length === 0 ? `Widget '${appId}' declares no sizes.` : null) : null;
  if (reason !== null) {
    return <UnavailableWidget appId={appId} stamp={stamp} reason={reason} />;
  }
  const picked = widgetCardSize(threadId, appId, params);
  const size = picked && sizes.includes(picked) ? picked : sizes[0] ?? UNREAD_BOX_SIZE;
  const target: WidgetMenuTarget = {
    threadId,
    appId,
    params,
    label,
    // Blank until the widget is read: a single value, so no skeleton.
    name: label ?? app?.name ?? '',
    reusable: app?.reusable ?? false,
    originPluginId: app?.origin_plugin_id,
    builtIn: app?.built_in ?? false,
    size,
  };
  const pick = (next: WidgetSize) => void sizeWidgetCards({ appId, threadId, params }, target.name, next);
  const box = widgetBoxCss(size);
  const compact = size === 'small';
  return (
    <WidgetPanel
      class="widget-panel inline-widget"
      style={{ '--widget-box-width': box.width, '--widget-box-height': box.height }}
      stamp={stamp}
      inlineWidget={appId}
      target={target}
      icon={app?.icon}
      compact={compact}
      leading={app && (
        <>
          <WidgetSizePill name={target.name} sizes={sizes} current={size} onPick={pick} />
          {!compact && <AddToDashboardButton tile={target} />}
        </>
      )}
      frame={(onMenu) => (app ? (
        <WidgetBox
          appId={appId}
          params={params}
          reveal={app.reveal}
          place="inline"
          size={size}
          transparent={app.transparent}
          onMenu={onMenu}
        />
      ) : (
        // The frame waits on the manifest's `reveal`. The box holds the room.
        <div class="widget-box" data-size={size} />
      ))}
    />
  );
}
