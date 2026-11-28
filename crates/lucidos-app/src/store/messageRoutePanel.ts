import { signal } from '@preact/signals';
import type { Exchange, StoredEvent } from './thread-events';

type MessageRoutePanelSection = 'origin' | 'executor';
export interface MessageRoutePanelState {
  anchor: HTMLElement;
  exchange: Exchange;
  threadId: string;
  section: MessageRoutePanelSection;
  priorModel?: string;
  priorEffort?: string;
  pausedBy?: StoredEvent;
  readAt?: string;
}
export const messageRoutePanel = signal<MessageRoutePanelState | null>(null);
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
