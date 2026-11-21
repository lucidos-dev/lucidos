/** The entry chunk's way into `scrollState.ts`, which ships in the shell chunk.
 *
 *  Nothing the entry chunk reaches may import the transcript's scroll engine,
 *  or the whole engine rides in it. `startShell` provides the real module
 *  before `<App/>` first renders. See *scroll state handle* in
 *  `docs/glossary.md`.
 *
 *  - A call made before then is queued and replays in order on provide. A
 *    deep link at boot focuses its thread before the shell exists.
 *  - A call made after forwards synchronously, inside the caller. The focus
 *    retire in `focusThread` must land before the focus moves.
 *  - A question answers as an empty document until then, which is what the
 *    real query returns while nothing is drawn. */
import type * as ScrollStateModule from './scrollState';
import type { DeepLinkOptions, EventDeepLinkOptions } from './scrollState';

type ScrollState = typeof ScrollStateModule;

let scrollState: ScrollState | null = null;
const queued: ((module: ScrollState) => void)[] = [];

/** Called once, by `startShell`. */
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
