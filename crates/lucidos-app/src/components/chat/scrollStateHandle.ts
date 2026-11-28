import type * as ScrollStateModule from './scrollState';
import type { DeepLinkOptions, EventDeepLinkOptions } from './scrollState';

type ScrollState = typeof ScrollStateModule;

let scrollState: ScrollState | null = null;
const queued: ((module: ScrollState) => void)[] = [];

export function provideScrollState(module: ScrollState): void {
  scrollState = module;
  for (const call of queued.splice(0)) call(module);
}

function tell(call: (module: ScrollState) => void): void {
  if (scrollState) call(scrollState);
  else queued.push(call);
}

export function stopFollowingBottom(): void {
  tell((s) => s.stopFollowingBottom());
}

export function clearPendingEventScroll(): void {
  tell((s) => s.clearPendingEventScroll());
}

export function scrollToEventAndPulse(eventId: string, opts?: EventDeepLinkOptions): void {
  tell((s) => s.scrollToEventAndPulse(eventId, opts));
}

export function scrollToChangeAndPulse(changeId: string, opts?: DeepLinkOptions): void {
  tell((s) => s.scrollToChangeAndPulse(changeId, opts));
}

export function followSentMessage(): void {
  tell((s) => s.followSentMessage());
}

export function isEventInViewport(eventId: string): boolean {
  return scrollState?.isEventInViewport(eventId) ?? false;
}

export function isReplyEndInViewport(threadId: string): boolean {
  return scrollState?.isReplyEndInViewport(threadId) ?? false;
}

export function lastExchangeOf(threadId: string): HTMLElement | null {
  return scrollState?.lastExchangeOf(threadId) ?? null;
}
