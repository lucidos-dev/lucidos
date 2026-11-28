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
import { restoreAdmissionPhase } from './restoreAdmission';
import { requestsInFlight } from '../api/client/requestsInFlight';

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
  renderedExchangesLen: number;
  freshExchangesLen: number;
  eventCount: number;
  eventsLoaded: boolean;
  hasContentEvents: boolean;
  animating: boolean;
  contentChildCount: number | null;
  contentScrollHeight: number | null;
}

export function classifyThreadRender(p: ThreadRenderProbe): ThreadRenderClass {
  if (!p.hasContentEvents) return 'genuinely-empty';

  if (p.renderedExchangesLen === 0 && p.freshExchangesLen > 0) return 'missed-rerender';

  if (p.renderedExchangesLen === 0) return 'empty-render';

  if (p.contentChildCount === 0) return 'dom-missing';

  return 'content-present';
}

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

/** The probe for a focused thread the store does not hold yet, such as a cold
 *  notification open still on its skeleton. Restore admission and the shell's
 *  requests are the two owners that can hold it, so it names both. Counts and
 *  ages only, never request paths. A temporary measure: see
 *  docs/temporary-measures.md § The render probe's absent-target report. */
export function reportAbsentThreadRender(threadId: string): void {
  if (!isWebKit()) return;
  const requests = requestsInFlight();
  postClientLog('render', 'thread_render_probe', {
    class: 'target-absent',
    thread_id: threadId,
    admission_phase: restoreAdmissionPhase(),
    requests_in_flight: requests.count,
    oldest_request_ms: requests.oldestMs,
  });
}
