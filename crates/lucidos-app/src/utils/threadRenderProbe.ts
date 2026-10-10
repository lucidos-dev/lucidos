/** Thread-render diagnostic probe — classifies WHY a focused thread's body can be
 *  blank while its summary (title / pin / compose / status) renders fine. The
 *  data is confirmed present in the DB for every report of this bug, so the
 *  failure is downstream of the store: either the view never rendered the
 *  exchanges, or it rendered them and WebKit didn't paint the layer.
 *
 *  A paint loss is INVISIBLE to JS (the DOM is fully built and laid out; only the
 *  compositor texture is stale), so this probe can't assert "the screen is blank"
 *  directly. Instead it captures the discriminating facts at a fixed delay after a
 *  thread settles and folds them into a single class, so the engine.log breadcrumb
 *  from the next repro tells us which of the three mechanisms it was:
 *
 *    - `missed-rerender`  — the store HAS exchanges now (`freshExchangesLen > 0`)
 *      but the last render produced none (`renderedExchangesLen === 0`): the view
 *      didn't re-render after the post-load `threadMap` write (stale subscription).
 *    - `empty-render`     — content events exist but exchanges fold to zero in BOTH
 *      the last render and a fresh recompute: a grouping / corrupted-Map gap (the
 *      domain of the `rebuildCorruptedThreadEvents` watchdog).
 *    - `dom-missing`      — exchanges rendered (`renderedExchangesLen > 0`) but the
 *      content area has no DOM children: Preact committed the memo result but the
 *      element subtree isn't there (a commit / mount gap).
 *    - `content-present`  — exchanges rendered AND the content DOM is built with
 *      non-zero height. Indistinguishable in JS from a healthy paint, so a blank
 *      report landing here is COMPOSITOR PAINT LOSS (recovered by a real scroll) —
 *      the `forceWebKitRepaint*` machinery's domain.
 *    - `genuinely-empty`  — no content events at all (a legit empty / draft thread,
 *      not a bug). Callers skip the breadcrumb for this class.
 *
 *  Built for the `ios-pwa-blackout` investigation, which closed on paint loss
 *  and the repaint hardening that cures it, then kept to confirm exactly this:
 *  a later blank-render regression. It is live again for
 *  `webkit-desktop-blank-thread`, the same blank on the packaged Mac app, and
 *  is registered against that investigation in docs/temporary-measures.md. The
 *  most symptom-specific of the client probes, but tiny and silent when idle. */

import { postClientLog } from './liveness';
import { isWebKit } from './platform';

export type ThreadRenderClass =
  | 'genuinely-empty'
  | 'missed-rerender'
  | 'empty-render'
  | 'dom-missing'
  | 'content-present';

export interface ThreadRenderProbe {
  threadId: string;
  channel: string;
  isCodingAgent: boolean;
  /** exchanges.length the LAST time ThreadView rendered this thread's body. */
  renderedExchangesLen: number;
  /** exchanges.length recomputed from the CURRENT store at probe time — a fresh
   *  fold, so a value that diverges from `renderedExchangesLen` proves the view
   *  is showing a stale render. */
  freshExchangesLen: number;
  eventCount: number;
  eventsLoaded: boolean;
  /** Whether the store holds any non-metadata (content) event — i.e. the thread
   *  SHOULD show a body. */
  hasContentEvents: boolean;
  /** `promptAnimating` — the compose→thread FLIP gate. On mobile this is always
   *  false (the FLIP is desktop-only); captured so a stuck-true value on any
   *  surface is visible. */
  animating: boolean;
  /** Child element count of the `.thread-content` scroll container, or null when
   *  the ref isn't mounted. Zero-with-exchanges = a commit/mount gap. */
  contentChildCount: number | null;
  /** Rendered height of the content container, or null when unmounted. Non-zero
   *  with content present rules out a collapsed/zero-size layout. */
  contentScrollHeight: number | null;
}

/** Classify a render probe into one of the discriminating buckets. Pure — no
 *  signal reads, no DOM, no I/O — so it is fully unit-testable. The order of the
 *  checks is the priority: a missed re-render is reported even when content events
 *  exist, because it is the more specific (and more actionable) diagnosis. */
export function classifyThreadRender(p: ThreadRenderProbe): ThreadRenderClass {
  // A thread with no content events is legitimately bodyless (fresh draft, a
  // thread carrying only ThreadStarted/metadata) — not the bug we hunt.
  if (!p.hasContentEvents) return 'genuinely-empty';

  // The store has exchanges now but the last render showed none → the view never
  // re-rendered to reflect the post-load store write. This is the "recovers on
  // scroll" stale-render mechanism.
  if (p.renderedExchangesLen === 0 && p.freshExchangesLen > 0) return 'missed-rerender';

  // Content events exist, yet a FRESH recompute still folds to zero exchanges →
  // a grouping / corrupted-Map gap (handled by rebuildCorruptedThreadEvents).
  if (p.renderedExchangesLen === 0) return 'empty-render';

  // Exchanges rendered, but the content area has no DOM children → Preact has the
  // memo result yet the subtree isn't mounted.
  if (p.contentChildCount === 0) return 'dom-missing';

  // Exchanges rendered and the DOM is built. Healthy to JS — a blank report here
  // is compositor paint loss, not a data/render fault.
  return 'content-present';
}

/** Whether a probe is worth a breadcrumb. Skip the legit-empty case so the log
 *  only carries threads that SHOULD show a body. Pure. */
export function shouldReportThreadRender(cls: ThreadRenderClass): boolean {
  return cls !== 'genuinely-empty';
}

/** Fire the diagnostic breadcrumb for a render probe. WebKit-only, and skips the
 *  genuinely-empty class. Fire-and-forget — `postClientLog` never throws. Lands as
 *  an engine.log `[Client/render] thread_render_probe …` line.
 *
 *  The surface is the ENGINE rather than the iOS PWA, because the blank came
 *  back on the packaged Mac app, which is a WKWebView. A Blink or Gecko client
 *  still logs nothing. The cost is one line per thread open on a WebKit client,
 *  which is why it is registered as a temporary measure: see
 *  docs/temporary-measures.md § `webkit-desktop-blank-thread`. */
export function reportThreadRenderProbe(probe: ThreadRenderProbe): void {
  if (!isWebKit()) return;
  const cls = classifyThreadRender(probe);
  if (!shouldReportThreadRender(cls)) return;
  postClientLog('render', 'thread_render_probe', {
    class: cls,
    thread_id: probe.threadId,
    channel: probe.channel,
    is_coding_agent: probe.isCodingAgent,
    rendered_exchanges: probe.renderedExchangesLen,
    fresh_exchanges: probe.freshExchangesLen,
    event_count: probe.eventCount,
    events_loaded: probe.eventsLoaded,
    has_content_events: probe.hasContentEvents,
    animating: probe.animating,
    content_child_count: probe.contentChildCount,
    content_scroll_height: probe.contentScrollHeight,
  });
}
