import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { OverflowMenu } from '../shared/OverflowMenu';
import { isMenuKey, useObjectMenu } from '../shared/useObjectMenu';
import type { ViewportPoint } from '../../hooks/useAnchoredPopover';
import { readAppsById } from '../../store/actions/appsById';
import { appById } from '../../store/appsById';
import { widgetCardSize } from '../../store/widgetCardSizes';
import type { App, WidgetSize } from '../../store/types';
import type { WidgetParams } from '../../utils/widgetParams';
import { widgetRefusal } from '../../utils/widgetEmbed';
import { widgetBoxCss } from '../../utils/dashboardGeometry';
import type { WidgetMenuTarget } from './WidgetMenu';
import { WidgetMenuItems } from './widgetMenuChunks';
import { WidgetBox } from './WidgetBox';

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
    <div class="inline-widget inline-widget-gone" data-event-id={stamp} data-inline-widget={appId}>
      {WIDGET_UNAVAILABLE_TEXT}
      {reason && <div class="inline-widget-gone-reason">{asSentence(reason)}</div>}
    </div>
  );
}

/** An inline widget's card: its widget box, bare, as a dashboard tile draws
 *  it. A right-click or a long press anywhere on it opens the widget menu,
 *  inside the frame included (`frame` gets `onMenu`). The click a hold pairs
 *  is swallowed in capture, so the widget under the finger does not also take
 *  it. The card is a focus stop, and the menu key or Shift+F10 opens the
 *  menu from the keyboard. `stamp` is the card's deep-link stamp, and `style`
 *  its box geometry. */
function WidgetCard({ style, stamp, target, frame }: {
  style: Record<string, string>;
  stamp?: string;
  target: WidgetMenuTarget;
  frame: (onMenu: (at?: ViewportPoint) => void) => ComponentChildren;
}) {
  const [card, setCard] = useState<HTMLDivElement | null>(null);
  const menu = useObjectMenu();
  const onMenu = (at?: ViewportPoint) => {
    if (card) menu.open(card, at);
  };
  return (
    <div
      ref={setCard}
      class={target.size === 'xl' ? 'inline-widget inline-widget-xl' : 'inline-widget'}
      role="group"
      aria-label={target.name || undefined}
      aria-haspopup="menu"
      tabIndex={0}
      style={style}
      data-event-id={stamp}
      data-inline-widget={target.appId}
      {...menu.press}
      onClickCapture={menu.onClick}
      onKeyDown={(e) => {
        if (!card || !isMenuKey(e)) return;
        e.preventDefault();
        menu.open(card);
      }}
    >
      {frame(onMenu)}
      <OverflowMenu
        ariaLabel={`${target.name} widget actions`}
        hostOpener={{ ref: menu.opener, trigger: false }}
        itemsChunk={WidgetMenuItems}
        items={(ctx) => <WidgetMenuItems ctx={ctx} target={target} />}
      />
    </div>
  );
}

/** The size a card holds its room at until its widget is read. */
const UNREAD_BOX_SIZE: WidgetSize = 'medium';

/** An *inline widget* (ADR 0466): a widget in a turn, drawn as its bare
 *  *widget box*, the same box its dashboard tile draws (ADR 0432). A reply
 *  draws one from its embed, and an old `WidgetShown` still draws one at its
 *  turn. `eventId` is an event of that turn.
 *
 *  The card starts at the widget's default size, and the size row of its menu
 *  picks another of the sizes it declares. A pick writes `InlineWidgetSized`,
 *  and the card draws the newest one the thread holds. It resizes the box and
 *  keeps the frame.
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
    sizes,
    size,
  };
  const box = widgetBoxCss(size);
  return (
    <WidgetCard
      style={{ '--widget-box-width': box.width, '--widget-box-height': box.height }}
      stamp={stamp}
      target={target}
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
