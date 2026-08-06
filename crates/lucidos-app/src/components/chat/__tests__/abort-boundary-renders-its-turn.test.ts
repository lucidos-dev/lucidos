import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

/**
 * **No turn renders as zero content.**
 *
 * An abort / cancel boundary is a statement about the turn that ENDED, not a
 * promise that nothing follows it. Work can legitimately land under one, and
 * the sharpest case is an event-wait wake: its anchor (a `UserPromptInjected`
 * written beside the resolution) is routed by `request_event_id`, and when no
 * exchange owns that id the fold drops its whole turn into whichever boundary
 * is current. If that boundary is an abort, the turn lives there as steps.
 *
 * `showResponsePanel` used to exclude `isAbortPanel` and `isCancelPanel`
 * unconditionally, so those steps were never drawn. On 2026-08-06 that made a
 * turn invisible which had applied a change to main, spawned a coding-agent
 * sub-thread and written a full per-project summary; the UI showed "Response
 * interrupted" and a Continue button and nothing else (real thread ebc787a4).
 *
 * A stepless boundary must still render bare, which is the common case and the
 * one the panel was written for, so the exception is gated on the exchange
 * actually having acquired something.
 *
 * **2026-08-06 follow-up.** "Something" was `hasEvents`, and that let the gate
 * back open too far. A boundary also picks up the DRAIN of whatever the teardown
 * killed, and a coding-agent subprocess signs off with a bare `"\n\n"`;
 * `exchangeResponseEvents` turns that into a `text` event, so `hasEvents` was
 * true while `renderResponseEvents` (which needs `evt.md?.trim()`) drew nothing.
 * The switch-teardown boundary got an empty response panel whose only visible
 * content was a status badge reading "Working" over a stopped engine. The gate
 * therefore asks `hasRenderableResponseContent`, the mirror of what the renderer
 * will actually draw. Both directions matter and both are pinned below: the
 * exclusions must not be dropped, and the gate must not be loosened back.
 */
describe('an abort or cancel boundary renders a turn that landed under it', () => {
  it('showResponsePanel admits a terminated boundary that has content', () => {
    const fnMatch = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m);
    expect(fnMatch, 'ChatExchangeImpl function not found').not.toBeNull();
    const fn = fnMatch![0];

    // The exception itself: content-bearing, and only for the two boundaries.
    expect(fn).toMatch(
      /isTerminatedContinuation\s*=\s*\(isAbortPanel \|\| isCancelPanel\)\s*&&\s*\(hasResponse \|\| hasRenderableResponseContent\(events\)\)/,
    );
    // And it is what relaxes both exclusions, rather than them being dropped.
    expect(fn).toMatch(/showResponsePanel\s*=[^;]*\(!isAbortPanel \|\| isTerminatedContinuation\)/);
    expect(fn).toMatch(/showResponsePanel\s*=[^;]*\(!isCancelPanel \|\| isTerminatedContinuation\)/);
  });

  /** `hasEvents` counts a whitespace-only text event; the renderer does not.
   *  Reverting to it brings back the empty "Working" panel. The whole statement
   *  is checked, not its first line: the gate wraps. */
  it('does not gate the exception on the bare event count', () => {
    const fn = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m)![0];
    const stmt = fn.match(/const isTerminatedContinuation[\s\S]*?;/)!;
    expect(stmt[0]).not.toContain('hasEvents');
  });

  it('does not simply drop the boundary exclusions', () => {
    const fnMatch = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m);
    const fn = fnMatch![0];
    const line = fn.split('\n').find((l: string) => l.includes('const showResponsePanel'))!;
    // A bare `!isAbortPanel` would be the pre-2026-08-06 behaviour; a missing
    // mention altogether would render an empty panel under every stepless
    // "Response interrupted" card. Both are wrong, so the gate must name the
    // boundary in its guarded form.
    expect(line).toContain('!isAbortPanel || isTerminatedContinuation');
    expect(line).toContain('!isCancelPanel || isTerminatedContinuation');
  });
});
