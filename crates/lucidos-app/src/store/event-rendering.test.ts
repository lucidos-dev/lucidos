import { describe, it, expect } from 'vitest';
import {
  drawsResponseRow, getCollapsedVisibleEvents, headClampApplies, liveStepInBody, responseBody, rowsDrawnByClamp,
  type BodySection,
} from './event-rendering';
import type { ResponseEvent, StepOutcome } from './types';

const step = (outcome: StepOutcome): ResponseEvent => ({
  type: 'step',
  description: outcome === 'pending' ? 'Thinking' : 'Read file',
  outcome,
});
const text = (md: string): ResponseEvent => ({ type: 'text', md });

const body = (events: ResponseEvent[], showSteps = true, showDetails = true, rowsHidden = 0) =>
  responseBody(events, { showSteps, showDetails, rowsHidden });

/** The live step `ChatExchange` marks, so the header label can read where it
 *  sits. It names the FIRST pending row the body DRAWS, by its index in the
 *  turn's events. */
describe('liveStepInBody', () => {
  it('names the pending row by its index in the turn', () => {
    expect(liveStepInBody(body([text('hi'), step('success'), step('pending')]))).toBe(2);
  });

  it('names the FIRST pending row when parallel calls leave several', () => {
    // The "Working" label sits above every row, so the first pending row is the
    // first one the reader meets coming down from it.
    expect(liveStepInBody(body([step('pending'), step('pending')]))).toBe(0);
  });

  it('is -1 when steps are hidden, so the label carries the shimmer', () => {
    expect(liveStepInBody(body([text('hi'), step('pending')], false))).toBe(-1);
  });

  it('is -1 when no drawn step is pending', () => {
    expect(liveStepInBody(body([step('success'), step('error'), text('done')]))).toBe(-1);
    // A step killed mid-call is terminal; a blocked one waits on the reader.
    expect(liveStepInBody(body([step('success'), step('unfinished')]))).toBe(-1);
    expect(liveStepInBody(body([step('success'), step('blocked')]))).toBe(-1);
    expect(liveStepInBody(body([text('just text')]))).toBe(-1);
    expect(liveStepInBody(body([]))).toBe(-1);
  });

  it('is -1 for a pending step the full response hides', () => {
    // Details off keeps only what follows the last prose chunk.
    expect(liveStepInBody(body([text('a'), step('pending'), text('b')], true, false))).toBe(-1);
  });
});

/** The rows a body draws, flattened: every open row's events plus every marker. */
function drawn(sections: BodySection[]): ResponseEvent[] {
  return sections.flatMap(s => s.rows.flatMap((r): ResponseEvent[] => {
    if (r.kind === 'marker') return [r.event];
    if (!r.open) return [];
    return r.kind === 'text' ? [r.event] : r.steps.map(x => x.event);
  }));
}

/** Every row's key, the identity Preact matches it by. */
function keys(sections: BodySection[]): string[] {
  return sections.flatMap(s => s.rows.map(r => `${s.key}/${r.kind}${r.key}`));
}

/** The body the two toggles roll. Each toggle state must still DRAW what it
 *  drew when hiding meant not rendering, and no toggle may move a row's key:
 *  a moved key remounts the row, which then snaps instead of rolling. */
describe('responseBody', () => {
  const image: ResponseEvent = { type: 'image', base64: '', mime_type: 'image/png' };
  const turns: Record<string, ResponseEvent[]> = {
    'prose, steps, prose': [text('plan'), step('success'), step('success'), text('done')],
    'a coding-agent turn': [text('\n'), step('success'), text('\n'), step('success'), text('look'), text(' '), step('pending')],
    'a marker among steps': [text('first'), step('success'), image, step('success'), text('last'), step('success')],
    'sectioned': [text('a'), step('success'), { type: 'section_break', channel: 'main' }, step('success'), text('b')],
    'steps only': [step('success'), step('success')],
    'one answer': [step('success'), text('only answer')],
  };
  const settings = [
    [true, true], [true, false], [false, true], [false, false],
  ] as const;

  for (const [name, events] of Object.entries(turns)) {
    it(`draws what the old paths drew: ${name}`, () => {
      for (const [showSteps, showDetails] of settings) {
        for (const rowsHidden of [0, 1, 2]) {
          const clamped = headClampApplies(events, showDetails);
          const before = (clamped ? events.slice(rowsHidden) : getCollapsedVisibleEvents(events))
            .filter(e => drawsResponseRow(e, showSteps));
          expect(drawn(body(events, showSteps, showDetails, rowsHidden)), `${showSteps}/${showDetails}/${rowsHidden}`)
            .toEqual(before);
        }
      }
    });

    it(`keeps every row's key through both toggles: ${name}`, () => {
      const first = keys(body(events));
      for (const [showSteps, showDetails] of settings) {
        expect(keys(body(events, showSteps, showDetails))).toEqual(first);
      }
    });
  }

  it('runs steps together across the blank chunks between them', () => {
    const [section] = body(turns['a coding-agent turn']);
    expect(section.rows.map(r => r.kind)).toEqual(['steps', 'text', 'steps']);
  });

  it('splits a run at a marker, which neither toggle hides', () => {
    const [section] = body(turns['a marker among steps'], false, false);
    expect(section.rows.map(r => r.kind)).toEqual(['text', 'steps', 'marker', 'steps', 'text', 'steps']);
    expect(section.rows.find(r => r.kind === 'marker')).toMatchObject({ event: image });
  });

  it('marks a hidden run between two drawn chunks for the hairline, and no other', () => {
    const elided = (events: ResponseEvent[], showSteps: boolean, showDetails: boolean) =>
      body(events, showSteps, showDetails).flatMap(s => s.rows)
        .filter(r => r.kind === 'steps').map(r => r.kind === 'steps' && r.elided);
    const events = [text('plan'), step('success'), text('done'), step('success')];
    // Between the two chunks: marked. After the last one: nothing to mark off.
    expect(elided(events, false, true)).toEqual([true, false]);
    // Steps on draws the run itself.
    expect(elided(events, true, true)).toEqual([false, false]);
    // The full response off hides the chunk before the run, so no boundary.
    expect(elided(events, false, false)).toEqual([false, false]);
    // A marker on one side is not a chunk.
    expect(elided([text('a'), step('success'), { type: 'empty' }], false, true)).toEqual([false]);
  });

  it('keys a section by its first event', () => {
    expect(body(turns.sectioned).map(s => s.key)).toEqual([0, 3]);
  });

  it('keys a run by its first step even when the clamp cuts that step', () => {
    const events = [step('success'), step('success'), step('success'), text('done')];
    const [section] = body(events, true, true, 1);
    expect(section.rows[0]).toMatchObject({ kind: 'steps', key: 0 });
    expect(section.rows[0].kind === 'steps' && section.rows[0].steps.map(s => s.index)).toEqual([1, 2]);
  });
});

/** What the response body is DRAWING, which is what decides whether the turn
 *  has anything to fold. A turn that draws nothing and folds anyway hides
 *  nothing, yet lights its collapse control as if it did. Reported while a
 *  coding-agent turn was in flight, which is where a blank body lives longest. */
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
        created: '2026-08-10T11:00:00Z',
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
    // The reason this is an allow-list. `responseBody` draws a row for some
    // kinds and none for these. A deny-list ("anything that is not a blank
    // text") would count them as body content.
    // A question and a permission render as initiator-panel dividers with their
    // own fold, not as response rows; a `section_break` splits
    // `responseBody` into sections. A turn holding only these has an empty body.
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

/** What the render window budgets by. A row the clamped slice renders as
 *  `null` costs nothing and fills no height, so the window must not count it.
 *  One reported turn held 748 text events and 45 with prose. With steps hidden,
 *  a run of 352 tool calls drew nothing at all. */
describe('rowsDrawnByClamp', () => {
  const view = { showSteps: true, showDetails: true, folded: false };
  const silentRun = [text('\n\n'), step('success'), text('\n\n'), step('success')];

  it('marks each row the slice would draw, row for row', () => {
    const events = [text('the plan'), ...silentRun, text('done')];
    expect(rowsDrawnByClamp(events, view)).toEqual([true, false, true, false, true, true]);
  });

  it('marks a silent run as drawing nothing once steps are hidden', () => {
    const events = [text('the plan'), ...silentRun];
    expect(rowsDrawnByClamp(events, { ...view, showSteps: false }))
      .toEqual([true, false, false, false, false]);
  });

  it('marks nothing drawn in a folded turn, whose body is not mounted', () => {
    const events = [text('the plan'), ...silentRun, text('done')];
    expect(rowsDrawnByClamp(events, { ...view, folded: true }).some(Boolean)).toBe(false);
  });

  it('marks nothing drawn on the collapsed-prose path, which ignores the clamp', () => {
    // Two prose chunks and a step: turning details off draws only what follows
    // the last prose, whatever `rowsHidden` says.
    const events = [text('first'), step('success'), text('last')];
    expect(headClampApplies(events, false)).toBe(false);
    expect(rowsDrawnByClamp(events, { ...view, showDetails: false }).some(Boolean)).toBe(false);
  });

  it('keeps the clamp where details off drops no prose', () => {
    const events = [step('success'), text('only answer')];
    expect(headClampApplies(events, false)).toBe(true);
    expect(rowsDrawnByClamp(events, { ...view, showDetails: false })).toEqual([true, true]);
  });

  it('keeps the clamp whenever details are on', () => {
    expect(headClampApplies([text('first'), step('success'), text('last')], true)).toBe(true);
  });
});
