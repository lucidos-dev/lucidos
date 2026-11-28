import { render } from 'preact';
import { useLayoutEffect, useState } from 'preact/hooks';
import { COPY_ID_NONCE, WIDGET_EMBED_ATTR, WIDGET_EMBED_ERROR_ATTR, WIDGET_EMBED_SLOT_ATTR } from '../../utils/markdownMarkers';
import type { WidgetEmbed } from '../../generated/thread-event-wire';
import { InlineWidget, UnavailableWidget } from './InlineWidget';
import { embedRefusal } from '../../utils/widgetEmbed';
import { useMermaidBlocks } from '../shared/useMermaidBlocks';

const SLOT_MARK = `${WIDGET_EMBED_SLOT_ATTR}="${COPY_ID_NONCE}"`;
const SLOT_SELECTOR = `[${SLOT_MARK}]`;

export function hasWidgetEmbedSlot(html: string): boolean {
  return html.includes(SLOT_MARK);
}

function slotEmbed(slot: HTMLElement): WidgetEmbed | null {
  const raw = slot.getAttribute(WIDGET_EMBED_ATTR);
  if (raw === null) return null;
  try {
    return JSON.parse(raw) as WidgetEmbed;
  } catch {
    return null;
  }
}

export function useWidgetEmbedSlots(host: HTMLElement | null, html: string, final: boolean, threadId: string, eventId?: string): void {
  useLayoutEffect(() => {
    if (!host || !final) return undefined;
    const mounted = Array.from(host.querySelectorAll<HTMLElement>(SLOT_SELECTOR));
    for (const slot of mounted) {
      const embed = slotEmbed(slot);
      const syntaxError = slot.getAttribute(WIDGET_EMBED_ERROR_ATTR) ?? undefined;
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
              refusal={(app) => embedRefusal(app, embed, threadId)}
            />
          )
          : <UnavailableWidget reason={syntaxError} />,
        slot,
      );
    }
    return () => {
      for (const slot of mounted) render(null, slot);
    };
  }, [host, html, final, threadId, eventId]);
}

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
  useMermaidBlocks(host, html, final);
  return <div ref={setHost} class={['authored-html', className].filter(Boolean).join(' ')} data-text-seq={textSeq} dangerouslySetInnerHTML={{ __html: html }} />;
}
