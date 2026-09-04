import { describe, it, expect } from 'vitest';
import { drawsResponseRow, liveStepIndex, rendersLiveStep } from './event-rendering';
import type { ResponseEvent, StepOutcome } from './types';

const step = (outcome: StepOutcome): ResponseEvent => ({
  type: 'step',
  description: outcome === 'pending' ? 'Thinking' : 'Read file',
  outcome,
});
const text = (md: string): ResponseEvent => ({ type: 'text', md });

describe('rendersLiveStep', () => {
  it('is true when steps are expanded, panel not collapsed, and a pending step is drawn', () => {
    expect(rendersLiveStep(true, false, [text('hi'), step('pending')])).toBe(true);
  });

  it('is false when steps are hidden, even if a pending step exists', () => {
    // The "Show steps" collapsed-toggle state: the live step draws no row, so
    // the "Working" label must carry the shimmer instead.
    expect(rendersLiveStep(false, false, [text('hi'), step('pending')])).toBe(false);
  });

  it('is false when the response panel is collapsed, even with a pending step', () => {
    // Collapse hides the whole steps body (only the header status shows), so the
    // pending step draws no shimmer and the label must carry it instead.
    expect(rendersLiveStep(true, true, [text('hi'), step('pending')])).toBe(false);
  });

  it('is false when steps are expanded but every visible step has resolved', () => {
    expect(rendersLiveStep(true, false, [step('success'), step('error'), text('done')])).toBe(false);
  });

  it('is false for an unfinished step: the turn died, nothing is running', () => {
    // A step killed mid-call is TERMINAL, not live. If it counted as a live
    // step it would suppress the "Working"/status shimmer on a dead turn and
    // (via `.running-shimmer`) animate a row nothing is working on.
    expect(rendersLiveStep(true, false, [step('unfinished')])).toBe(false);
    expect(rendersLiveStep(true, false, [step('success'), step('unfinished')])).toBe(false);
  });

  it('is false when there are no step events at all', () => {
    expect(rendersLiveStep(true, false, [text('just text')])).toBe(false);
    expect(rendersLiveStep(true, false, [])).toBe(false);
  });
});

/** The index `ChatExchange` marks a row by, so the header label can read where
 *  that row sits. It has to agree with `rendersLiveStep` on what a live row is,
 *  and it has to name the FIRST one. */
describe('liveStepIndex', () => {
  it('is the position of the pending row in the drawn list', () => {
    expect(liveStepIndex(true, false, [text('hi'), step('success'), step('pending')])).toBe(2);
  });

  it('names the FIRST pending row when parallel calls leave several', () => {
    // The "Working" label sits above every row, so the first pending row is the
    // first one the reader meets coming down from it. Naming the last would let
    // the label shimmer over a visible row above it.
    expect(liveStepIndex(true, false, [step('pending'), step('pending')])).toBe(0);
  });

  it('is -1 wherever no row is drawn, matching rendersLiveStep', () => {
    const pending = [text('hi'), step('pending')];
    expect(liveStepIndex(false, false, pending)).toBe(-1);
    expect(liveStepIndex(true, true, pending)).toBe(-1);
    expect(liveStepIndex(true, false, [step('success'), step('blocked')])).toBe(-1);
    expect(liveStepIndex(true, false, [])).toBe(-1);
  });
});

/** What the response body is DRAWING, which is what decides whether the turn
 *  has anything to fold. The fold swaps the body for a `⋯` stub, so a turn that
 *  draws nothing and folds anyway swaps nothing for a mark: it does not
 *  collapse, it APPEARS. Reported while a coding-agent turn was in flight,
 *  which is where a blank body lives longest. */
describe('drawsResponseRow', () => {
  it('draws a text event only when it has visible text', () => {
    // A whitespace-only chunk is the norm, not a curiosity: one is pushed for
    // every `CodingAgentTextStreamed`, and a torn-down subprocess signs off
    // with a bare "\n\n". Counting those is how `events.length` runs ahead of
    // anything on screen.
    expect(drawsResponseRow(text('an answer'), false)).toBe(true);
    expect(drawsResponseRow(text('  \n\n  '), false)).toBe(false);
    expect(drawsResponseRow(text(''), true)).toBe(false);
  });

  it('draws step mechanics only while the steps control is on', () => {
    // A turn that has emitted only steps shows an EMPTY body to a reader who
    // turned `stepsExpanded` off: nothing to fold until the first row lands.
    for (const outcome of ['pending', 'success', 'error', 'unfinished'] as StepOutcome[]) {
      expect(drawsResponseRow(step(outcome), true), outcome).toBe(true);
      expect(drawsResponseRow(step(outcome), false), outcome).toBe(false);
    }
  });

  it('draws every marker unconditionally, whatever the steps control says', () => {
    // Markers are not mechanics (`isStepMechanics`): each records that a thing
    // happened, and no toggle hides one.
    const markers: ResponseEvent[] = [
      { type: 'image', base64: '', mime_type: 'image/png' },
      {
        type: 'checkpoint',
        checkpoint_id: 'c1',
        command: 'rm -rf build',
        summary: 'Removed the build directory',
        reverted: false,
        restores: 0,
        removes: 3,
      },
      {
        type: 'event_wait',
        wait_id: 'w1',
        subscriptions: [{ event_type: 'ChangeApplied' }],
        reason: 'waiting for the apply',
        expires_at: '2026-08-10T12:00:00Z',
        state: 'waiting',
      },
      { type: 'empty' },
    ];
    for (const m of markers) {
      expect(drawsResponseRow(m, true), m.type).toBe(true);
      expect(drawsResponseRow(m, false), m.type).toBe(true);
    }
  });

  it('draws nothing for the kinds the response renderer has no arm for', () => {
    // The reason this is an allow-list. `renderResponseEvents` draws five of the
    // nine kinds and falls through to `null` for the rest, so a deny-list
    // ("anything that is not a blank text") would count these as body content.
    // A question and a permission render as initiator-panel dividers with their
    // own fold, not as response rows; a `section_break` is consumed by
    // `splitEventSections`. A turn holding only these has an empty body.
    const undrawn: ResponseEvent[] = [
      { type: 'section_break', channel: 'main' },
      { type: 'question', tool_use_id: 't1', question: 'Which?', options: [] },
      { type: 'permission', request_id: 'r1', tool_use_id: 't2', tool_name: 'Bash', input: {}, summary: 'ls' },
    ];
    for (const e of undrawn) {
      expect(drawsResponseRow(e, true), e.type).toBe(false);
      expect(drawsResponseRow(e, false), e.type).toBe(false);
    }
  });
});
