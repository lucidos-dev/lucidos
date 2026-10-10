import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { OverflowMenu } from '../shared/OverflowMenu';
import { OpenInCanvasIcon } from '../shared/icons';
import { AppIcon } from '../shared/AppIcon';
import { rereadThreadWidgets } from '../../store/actions/widgets';
import { openWidgetInCanvas } from '../../store/actions/widget-actions';
import { appliedThreadWidgetTicket, threadWidget, threadWidgetsFor } from '../../store/widgets';
import type { AppReveal } from '../../store/types';
import type { WidgetParams } from '../../utils/widgetParams';
import { widgetMenuItems, type WidgetMenuTarget } from './WidgetMenu';
import { WidgetFrame } from './WidgetFrame';

/** A widget's bar: its app icon, its name, the word "widget", and its
 *  actions. The thread's widget card and the dropped open shelf panel both
 *  draw it. The icon slot stays empty until the name lands. */
export function WidgetBar({ target, icon, leading, trailing }: {
  target: WidgetMenuTarget;
  icon?: string;
  leading?: ComponentChildren;
  trailing?: ComponentChildren;
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
        <OverflowMenu ariaLabel={`${target.name} widget actions`} items={(ctx) => widgetMenuItems(ctx, target)} />
        {trailing}
      </span>
    </div>
  );
}

/** A widget instance's card at the turn that showed it (`WidgetShown`, ADRs
 *  0407, 0415): its bar over its frame. Unpinning it from the shelf leaves the
 *  card. Its name comes from the thread's widgets, which the title row loads;
 *  the event's own label wins. */
export function WidgetCard({ threadId, appId, params, label, eventId }: {
  threadId: string;
  appId: string;
  params?: WidgetParams;
  label?: string;
  eventId?: string;
}) {
  const widgets = threadWidgetsFor(threadId);
  const widget = threadWidget(threadId, appId, params);
  // A read before this widget was shown does not name it. Only a read
  // started after the card noticed may say the widget is gone.
  const missing = widgets.status === 'loaded' && !widget;
  const [askedAt, setAskedAt] = useState<number | null>(null);
  useEffect(() => {
    if (missing && askedAt === null) setAskedAt(rereadThreadWidgets(threadId));
  }, [missing, askedAt, threadId]);
  if (missing && askedAt !== null && appliedThreadWidgetTicket(threadId) >= askedAt) {
    return (
      <div class="widget-card widget-card-inline widget-card-gone" data-event-id={eventId}>
        This widget no longer exists.
      </div>
    );
  }
  const target: WidgetMenuTarget = {
    threadId,
    appId,
    params,
    label,
    // Blank until the thread's widgets land: a single value, so no skeleton.
    name: label ?? widget?.name ?? '',
    reusable: widget?.reusable ?? false,
    builtIn: widget?.built_in ?? false,
    pinned: widget?.pinned ?? false,
  };
  return (
    <div class="widget-card widget-card-inline" data-event-id={eventId}>
      <WidgetBar target={target} icon={widget?.icon} />
      <ClippedFrame appId={appId} params={params} name={target.name} reveal={widget?.reveal ?? 'on-load'} />
    </div>
  );
}

/** The inline frame inside a box about one phone screen tall. The frame keeps
 *  its full height, so it never scrolls inside the transcript. A widget taller
 *  than the box is clipped, and says so with a fade and an Expand button. */
function ClippedFrame({ appId, params, name, reveal }: {
  appId: string;
  params?: WidgetParams;
  name: string;
  reveal: AppReveal;
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
    <div ref={setClip} class="widget-card-clip">
      <WidgetFrame appId={appId} params={params} reveal={reveal} place="inline" onHeight={setHeight} />
      {clipped && (
        <div class="widget-card-fade">
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
