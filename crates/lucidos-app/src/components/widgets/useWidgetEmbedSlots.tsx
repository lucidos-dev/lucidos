import { render } from 'preact';
import { useLayoutEffect, useState } from 'preact/hooks';
import { COPY_ID_NONCE, WIDGET_EMBED_ATTR, WIDGET_EMBED_SLOT_ATTR } from '../../utils/markedConfig';
import type { WidgetEmbed } from '../../generated/thread-event-wire';
import { InlineWidget, UnavailableWidget } from './InlineWidget';
import { embedDraws } from '../../utils/widgetEmbed';

const SLOT_SELECTOR = `[${WIDGET_EMBED_SLOT_ATTR}="${COPY_ID_NONCE}"]`;

function slotEmbed(slot: HTMLElement): WidgetEmbed | null {
  const raw = slot.getAttribute(WIDGET_EMBED_ATTR);
  if (raw === null) return null;
  try {
    return JSON.parse(raw) as WidgetEmbed;
  } catch {
    return null;
  }
}

/** Mount an *inline widget* into each embed slot the markdown renderer wrote
 *  in `host` (ADRs 0415, 0466). Only while `final`: a streaming reply replaces
 *  its HTML on every token, so it shows each embed's label instead. A
 *  re-render with the same `html` keeps its slots, and so its frames.
 *  `eventId` is the turn's starter, which a pin records. */
export function useWidgetEmbedSlots(host: HTMLElement | null, html: string, final: boolean, threadId: string, eventId?: string): void {
  useLayoutEffect(() => {
    if (!host || !final) return undefined;
    const mounted = Array.from(host.querySelectorAll<HTMLElement>(SLOT_SELECTOR));
    for (const slot of mounted) {
      const embed = slotEmbed(slot);
      slot.textContent = '';
      render(
        embed
          ? (
            <InlineWidget
              threadId={threadId}
              appId={embed.app_id}
              params={embed.params}
              label={embed.label}
              eventId={eventId}
              mayDraw={(app) => embedDraws(app, embed, threadId)}
            />
          )
          : <UnavailableWidget />,
        slot,
      );
    }
    return () => {
      for (const slot of mounted) render(null, slot);
    };
  }, [host, html, final, threadId, eventId]);
}

/** Reply HTML whose widget embeds mount as inline widgets once the text is
 *  final. `eventId` is the turn's starter, where a pin's "Show in thread"
 *  lands. */
export function ReplyHtml({ html, final, threadId, eventId, class: className, textSeq }: {
  html: string;
  final: boolean;
  threadId: string;
  eventId?: string;
  class?: string;
  textSeq?: number;
}) {
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  useWidgetEmbedSlots(host, html, final, threadId, eventId);
  return <div ref={setHost} class={className} data-text-seq={textSeq} dangerouslySetInnerHTML={{ __html: html }} />;
}
