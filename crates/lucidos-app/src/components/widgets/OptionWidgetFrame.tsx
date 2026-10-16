import { useEffect } from 'preact/hooks';
import type { WidgetEmbed } from '../../generated/thread-event-wire';
import { appById } from '../../store/appsById';
import { readAppsById } from '../../store/actions/appsById';
import { WIDGET_UNAVAILABLE_TEXT } from './InlineWidget';
import { WidgetFrame, defaultWidgetHeight } from './WidgetFrame';

/** The frame an option's *widget embed* draws inside its box: its widget, with
 *  its params, once the widget's manifest is read. The engine checked the
 *  embed before the card was asked, so a missing or non-widget id is all that
 *  reads "This widget is not available". A reply's embed is an inline widget
 *  instead (`useWidgetEmbedSlots.tsx`). */
export function OptionWidgetFrame({ embed }: { embed: WidgetEmbed }) {
  const read = appById(embed.app_id);
  useEffect(() => {
    if (read.status === 'not-loaded') void readAppsById([embed.app_id]);
  }, [read.status, embed.app_id]);
  const app = read.status === 'loaded' ? read.data : undefined;
  if (read.status === 'failed' || app === null || (app && app.kind !== 'widget')) {
    return <div class="widget-embed-unavailable" data-widget-embed={embed.app_id}>{WIDGET_UNAVAILABLE_TEXT}</div>;
  }
  return (
    <div class="widget-embed widget-embed-option" data-widget-embed={embed.app_id}>
      {app ? (
        <WidgetFrame appId={embed.app_id} params={embed.params} reveal={app.reveal} place="option" />
      ) : (
        // The frame waits on the manifest's `reveal`. Its box holds the room.
        <div class="widget-frame widget-frame-option" style={{ height: `${defaultWidgetHeight('option')}px` }} />
      )}
    </div>
  );
}
