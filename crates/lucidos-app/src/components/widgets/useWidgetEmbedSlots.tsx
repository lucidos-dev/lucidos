import { render } from 'preact';
import { useLayoutEffect, useState } from 'preact/hooks';
import { COPY_ID_NONCE, WIDGET_EMBED_ATTR, WIDGET_EMBED_SLOT_ATTR } from '../../utils/markedConfig';
import type { WidgetEmbed } from '../../generated/thread-event-wire';
import { OverflowMenu } from '../shared/OverflowMenu';
import { appById } from '../../store/appsById';
import { WidgetEmbedFrame, WIDGET_UNAVAILABLE_TEXT } from './WidgetEmbedFrame';
import { embedMenuItems } from './WidgetMenu';

const SLOT_SELECTOR = `[${WIDGET_EMBED_SLOT_ATTR}="${COPY_ID_NONCE}"]`;

/** A reply's widget embed: its frame, with a bar naming it and a menu. */
function ReplyWidgetEmbed({ threadId, embed }: { threadId: string; embed: WidgetEmbed }) {
  // The frame draws only once the widget is read, so its name is known here.
  const read = appById(embed.app_id);
  const name = embed.label ?? (read.status === 'loaded' && read.data ? read.data.name : embed.app_id);
  return (
    <WidgetEmbedFrame embed={embed} place="embed" threadId={threadId}>
      <div class="widget-embed-bar">
        <span class="widget-embed-name">{name}</span>
        <OverflowMenu
          ariaLabel={`${name} widget actions`}
          items={(ctx) => embedMenuItems(ctx, { threadId, appId: embed.app_id, params: embed.params, label: embed.label, name })}
        />
      </div>
    </WidgetEmbedFrame>
  );
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

/** Mount a widget into each embed slot the markdown renderer wrote in `host`
 *  (ADR 0415). Only while `final`: a streaming reply replaces its HTML on
 *  every token, so it shows each embed's label instead. A re-render with the
 *  same `html` keeps its slots, and so its frames. */
export function useWidgetEmbedSlots(host: HTMLElement | null, html: string, final: boolean, threadId: string): void {
  useLayoutEffect(() => {
    if (!host || !final) return undefined;
    const mounted = Array.from(host.querySelectorAll<HTMLElement>(SLOT_SELECTOR));
    for (const slot of mounted) {
      const embed = slotEmbed(slot);
      slot.textContent = '';
      render(
        embed
          ? <ReplyWidgetEmbed threadId={threadId} embed={embed} />
          : <div class="widget-embed-unavailable">{WIDGET_UNAVAILABLE_TEXT}</div>,
        slot,
      );
    }
    return () => {
      for (const slot of mounted) render(null, slot);
    };
  }, [host, html, final, threadId]);
}

/** Reply HTML whose widget embeds mount once the text is final. */
export function ReplyHtml({ html, final, threadId, class: className, textSeq }: {
  html: string;
  final: boolean;
  threadId: string;
  class?: string;
  textSeq?: number;
}) {
  const [host, setHost] = useState<HTMLDivElement | null>(null);
  useWidgetEmbedSlots(host, html, final, threadId);
  return <div ref={setHost} class={className} data-text-seq={textSeq} dangerouslySetInnerHTML={{ __html: html }} />;
}
