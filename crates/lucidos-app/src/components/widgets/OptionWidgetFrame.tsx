import { useEffect } from 'preact/hooks';
import type { WidgetEmbed } from '../../generated/thread-event-wire';
import { appById } from '../../store/appsById';
import { readAppsById } from '../../store/actions/appsById';
import { WIDGET_UNAVAILABLE_TEXT } from './InlineWidget';
import { WidgetFrame, defaultWidgetHeight } from './WidgetFrame';

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
        <div class="widget-frame widget-frame-option" style={{ height: `${defaultWidgetHeight('option')}px` }} />
      )}
    </div>
  );
}
