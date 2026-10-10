import type { ComponentChildren } from 'preact';
import { useEffect } from 'preact/hooks';
import type { WidgetEmbed } from '../../generated/thread-event-wire';
import { appById } from '../../store/appsById';
import { readAppsById } from '../../store/actions/appsById';
import type { App } from '../../store/types';
import { WIDGET_PARAMS_MAX_BYTES } from '@lucidos/engine-constants';
import { canonicalWidgetParams } from '../../utils/widgetParams';
import { WidgetFrame, defaultWidgetHeight } from './WidgetFrame';

/** What a *widget embed* draws when its widget cannot: in place, never
 *  refused, since a reply cannot be (ADR 0415). */
export const WIDGET_UNAVAILABLE_TEXT = 'This widget is not available';

/** Own keys only: `toString` is not a param a widget declared. */
function hasOwn(record: object, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(record, key);
}

/** Whether this embed may draw in this thread: a widget, reusable or made
 *  here, given only the params its manifest takes and every required one.
 *  The engine checks an option's embed before the card; a reply's embed has
 *  only this check. */
export function embedDraws(app: App, embed: WidgetEmbed, threadId: string): boolean {
  if (app.kind !== 'widget') return false;
  if (!app.reusable && app.origin_thread_id !== threadId) return false;
  const declared = app.params ?? {};
  const given = embed.params ?? {};
  if (Object.keys(given).some((name) => !hasOwn(declared, name))) return false;
  if (new TextEncoder().encode(canonicalWidgetParams(given)).length > WIDGET_PARAMS_MAX_BYTES) return false;
  return Object.entries(declared).every(([name, param]) => !param.required || hasOwn(given, name));
}

/** Where an embed draws. A reply's embed names its thread and draws only
 *  when `embedDraws` says so. An option's embed was checked by the engine
 *  before its card was asked. */
type EmbedPlacement = { place: 'option' } | { place: 'embed'; threadId: string };

/** The frame a *widget embed* draws: its widget, with its params, once the
 *  widget's manifest is read. A missing id, or a reply embed that may not
 *  draw, reads "This widget is not available". `children` are drawn beside
 *  the frame once it can draw, such as a reply embed's menu. */
export function WidgetEmbedFrame({ embed, children, ...placement }: EmbedPlacement & {
  embed: WidgetEmbed;
  children?: ComponentChildren;
}) {
  const { place } = placement;
  const read = appById(embed.app_id);
  useEffect(() => {
    if (read.status === 'not-loaded') void readAppsById([embed.app_id]);
  }, [read.status, embed.app_id]);
  const app = read.status === 'loaded' ? read.data : undefined;
  const refused = app && (placement.place === 'embed' ? !embedDraws(app, embed, placement.threadId) : app.kind !== 'widget');
  if (read.status === 'failed' || app === null || refused) {
    return <div class="widget-embed-unavailable" data-widget-embed={embed.app_id}>{WIDGET_UNAVAILABLE_TEXT}</div>;
  }
  return (
    <div class={`widget-embed widget-embed-${place}`} data-widget-embed={embed.app_id}>
      {app ? (
        <>
          {children}
          <WidgetFrame appId={embed.app_id} params={embed.params} reveal={app.reveal} place={place} />
        </>
      ) : (
        // The frame waits on the manifest's `reveal`. Its box holds the room.
        <div class={`widget-frame widget-frame-${place}`} style={{ height: `${defaultWidgetHeight(place)}px` }} />
      )}
    </div>
  );
}
