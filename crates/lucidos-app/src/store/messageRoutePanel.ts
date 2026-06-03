import { signal } from '@preact/signals';
import type { Exchange } from './thread-events';

// --- Message route panel (anchored popover for the route badge) ---
type MessageRoutePanelSection = 'origin' | 'executor';
export interface MessageRoutePanelState {
  anchor: HTMLElement;
  exchange: Exchange;
  threadId: string;
  section: MessageRoutePanelSection;
  /** Carried forward from prior exchanges so the panel can show model/effort
   *  before the current exchange's ResponseGenerated event arrives. */
  priorModel?: string;
  priorEffort?: string;
}
export const messageRoutePanel = signal<MessageRoutePanelState | null>(null);
/** Click semantics for the route badge: opens the panel for the given exchange +
 *  section, or closes it when re-clicking the same combination. Switching to a
 *  different exchange/section opens the panel with the new contents.
 *
 *  Identity is `(threadId, userSeq, section)` rather than the anchor DOM ref
 *  because the badge button can be re-rendered between clicks (streaming
 *  updates), which would replace the DOM node and silently defeat ref equality. */
export function toggleMessageRoutePanel(state: MessageRoutePanelState): void {
  const current = messageRoutePanel.value;
  if (
    current &&
    current.threadId === state.threadId &&
    current.exchange.userSeq === state.exchange.userSeq &&
    current.section === state.section
  ) {
    messageRoutePanel.value = null;
    return;
  }
  messageRoutePanel.value = state;
}
export function closeMessageRoutePanel(): void {
  messageRoutePanel.value = null;
}
