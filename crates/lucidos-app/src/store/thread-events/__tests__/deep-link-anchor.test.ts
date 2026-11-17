import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { readerHasReached, landingTarget, deepLinkAnchorForEvent, deepLinkTargetIndex, stampedEventIds } from '../exchange-anchors';
import type { Exchange } from '../exchange';
import type { StoredEvent } from '../thread-event-types';

/**
 * A deep-link resolves in the DOM by `data-event-id`, and only three kinds of
 * element ever carry one. `deepLinkAnchorForEvent` is what lets a link to an event that
 * carries none of them (any ordinary step, e.g. the `CodingAgentIdled` an event wait
 * usually matches) still land, by re-targeting it at the turn that holds it.
 *
 * The set of self-stamping ids is declared once in `stampedEventIds`; the
 * tripwires here fail if `ChatExchange` or `InlineWidget` grows a stamp without
 * declaring it.
 */

const evt = (type: string, id: string, over: Record<string, unknown> = {}): StoredEvent =>
  ({ type, _eventId: id, ...over }) as unknown as StoredEvent;

let seq = 0;
function exchange(starterId: string, stepIds: [string, string, Record<string, unknown>?][] = []): Exchange {
  return {
    userEvent: evt('MessageReceived', starterId, { content: 'hi' }),
    userSeq: ++seq,
    steps: stepIds.map(([type, id, fields]) => ({
      seq: ++seq,
      event: type === 'ResponseFailed'
        ? evt(type, id, { error: 'boom' })
        : evt(type, id, fields),
    })),
  } as unknown as Exchange;
}

describe('deepLinkAnchorForEvent', () => {
  it('returns the event itself when it is the turn starter', () => {
    const exchanges = [exchange('start-1'), exchange('start-2')];
    expect(deepLinkAnchorForEvent(exchanges, 'start-2')).toBe('start-2');
  });

  it('returns the event itself for a ResponseFailed, which stamps its own card', () => {
    const exchanges = [exchange('start-1', [['ResponseFailed', 'fail-1']])];
    expect(deepLinkAnchorForEvent(exchanges, 'fail-1')).toBe('fail-1');
  });

  /** The case the event-wait step hits: a terminator folded into a turn as a
   *  step, stamping nothing. Before this, the link resolved to no element and
   *  spent the whole 4s deadline before recovering to the bottom. */
  it('returns the containing turn for a step that stamps nothing', () => {
    const exchanges = [
      exchange('start-1'),
      exchange('start-2', [['CodingAgentToolCalled', 'tool-1'], ['CodingAgentIdled', 'idle-1']]),
    ];
    expect(deepLinkAnchorForEvent(exchanges, 'idle-1')).toBe('start-2');
    expect(deepLinkAnchorForEvent(exchanges, 'tool-1')).toBe('start-2');
  });

  /** A deep link lands on an old showing's inline widget, not on the
   *  message before it, so the widget's stamp must be declared. */
  it('returns the event itself for a WidgetShown, which stamps its own inline widget', () => {
    const exchanges = [exchange('start-1', [['ToolCalled', 'tool-1'], ['WidgetShown', 'widget-1']])];
    expect(deepLinkAnchorForEvent(exchanges, 'widget-1')).toBe('widget-1');
  });

  it('returns null when no turn holds the event', () => {
    expect(deepLinkAnchorForEvent([exchange('start-1')], 'elsewhere')).toBeNull();
  });

  /** **The bug this predicate exists for** (reported 2026-08-10). A background
   *  bash task completed while the turn started by a `UserQuestionAsked` was
   *  open, so grouping filed the completion under that question and the wake
   *  card's jump pulsed it. The turn did not cause the event, so the
   *  containing-turn inference is false and there is no anchor at all. */
  it('returns null for an event that merely landed in the open turn', () => {
    const exchanges = [
      exchange('question-1', [['BackgroundBashCompleted', 'bash-done-1']]),
    ];
    expect(deepLinkAnchorForEvent(exchanges, 'bash-done-1')).toBeNull();
  });

  /** Its own START is a different matter: the turn's `run_bash_background` call
   *  emitted it, so landing on that turn is honest. The pair is deliberately
   *  split rather than excluded together. */
  it('keeps the containing turn for the bash START, which the turn caused', () => {
    const exchanges = [
      exchange('start-1', [['BackgroundBashStarted', 'bash-start-1']]),
    ];
    expect(deepLinkAnchorForEvent(exchanges, 'bash-start-1')).toBe('start-1');
  });

  /** The unanchorable rule is about the containing-turn INFERENCE, so it never
   *  overrides an event that addresses itself. (No such event is in the set
   *  today; the ordering is pinned so adding one cannot silently break the
   *  stronger answer.) */
  it('still prefers a self-stamped element over the unanchorable rule', () => {
    const exchanges = [exchange('start-1', [['ResponseFailed', 'fail-1']])];
    expect(deepLinkAnchorForEvent(exchanges, 'fail-1')).toBe('fail-1');
  });

  /** A legacy row whose starter has no event id gives the link nothing to aim
   *  at. Saying null beats handing back an `undefined` that reads as a hit. */
  it('returns null when the containing turn has no stamped starter', () => {
    const stray = exchange('unused', [['CodingAgentIdled', 'idle-1']]);
    (stray.userEvent as { _eventId?: string })._eventId = undefined;
    expect(deepLinkAnchorForEvent([stray], 'idle-1')).toBeNull();
  });

  it('resolves an id collision to the most recent owner, like the grouping walk', () => {
    const exchanges = [
      exchange('start-1', [['CodingAgentIdled', 'dup']]),
      exchange('start-2', [['CodingAgentIdled', 'dup']]),
    ];
    expect(deepLinkAnchorForEvent(exchanges, 'dup')).toBe('start-2');
  });
});

/** Which turn the window must draw for a deep link to land. The window's floor
 *  moves to it (`deepLinkEdge`), rather than to the thread's first turn. */
describe('deepLinkTargetIndex', () => {
  it('finds the turn an event starts, a step of, or a card it stamps', () => {
    const exchanges = [
      exchange('start-1'),
      exchange('start-2', [['CodingAgentIdled', 'idle-1'], ['ResponseFailed', 'fail-1']]),
      exchange('start-3'),
    ];
    expect(deepLinkTargetIndex(exchanges, { kind: 'event', eventId: 'start-1' })).toBe(0);
    expect(deepLinkTargetIndex(exchanges, { kind: 'event', eventId: 'idle-1' })).toBe(1);
    expect(deepLinkTargetIndex(exchanges, { kind: 'event', eventId: 'fail-1' })).toBe(1);
    expect(deepLinkTargetIndex(exchanges, { kind: 'event', eventId: 'start-3' })).toBe(2);
  });

  /** An unanchorable event still names the turn it arrived in. Drawing that
   *  turn costs one turn, where a render-all costs the thread. */
  it('names the turn holding an unanchorable event', () => {
    const exchanges = [exchange('start-1'), exchange('start-2', [['BackgroundBashCompleted', 'bash-1']])];
    expect(deepLinkTargetIndex(exchanges, { kind: 'event', eventId: 'bash-1' })).toBe(1);
  });

  /** A fragment stamps no starter, so a link to one of its steps has nothing
   *  to land on. Paging must go on until the real turn merges in. */
  it('answers -1 for a step of a continuation fragment', () => {
    const fragment = { ...exchange('frag-1', [['CodingAgentIdled', 'idle-2']]), continuationFragment: true as const };
    expect(deepLinkTargetIndex([fragment, exchange('start-2')], { kind: 'event', eventId: 'idle-2' })).toBe(-1);
  });

  it('answers -1 for an event no loaded turn holds', () => {
    expect(deepLinkTargetIndex([exchange('start-1')], { kind: 'event', eventId: 'older' })).toBe(-1);
  });

  /** `scrollToChangeAndPulse` prefers the LAST stamped turn: the resolution
   *  card over the proposing turn. */
  it('finds the newest turn stamped with a change', () => {
    const proposing = exchange('start-1', [['ChangeProposed', 'prop-1', { change_id: 'c-1' }]]);
    const applied = {
      ...exchange('applied-1'),
      userEvent: evt('ChangeApplied', 'applied-1', { change_id: 'c-1' }),
    } as Exchange;
    expect(deepLinkTargetIndex([proposing, exchange('start-2')], { kind: 'change', changeId: 'c-1' })).toBe(0);
    expect(deepLinkTargetIndex([proposing, applied, exchange('start-3')], { kind: 'change', changeId: 'c-1' })).toBe(1);
    expect(deepLinkTargetIndex([proposing], { kind: 'change', changeId: 'c-2' })).toBe(-1);
  });

  /** A per-commit `ChangeProposed` carries an empty id and stamps nothing. */
  it('ignores a per-commit proposal with no change id', () => {
    const commit = exchange('start-1', [['ChangeProposed', 'prop-1', { change_id: '' }]]);
    expect(deepLinkTargetIndex([commit], { kind: 'change', changeId: '' })).toBe(-1);
  });
});

describe('stampedEventIds', () => {
  it('lists the starter, plus the failure card when the turn failed', () => {
    expect(stampedEventIds(exchange('start-1'))).toEqual(['start-1']);
    expect(stampedEventIds(exchange('start-1', [['ResponseFailed', 'fail-1']])))
      .toEqual(['start-1', 'fail-1']);
  });

  it("lists every old showing's inline widget in the turn", () => {
    expect(stampedEventIds(exchange('start-1', [['WidgetShown', 'w-1'], ['ToolCalled', 't-1'], ['WidgetShown', 'w-2']])))
      .toEqual(['start-1', 'w-1', 'w-2']);
  });

  it('omits an id the DOM would not carry', () => {
    const noId = exchange('unused');
    (noId.userEvent as { _eventId?: string })._eventId = undefined;
    expect(stampedEventIds(noId)).toEqual([]);
  });

  /**
   * Tripwire. `stampedEventIds` claims to enumerate every `data-event-id`
   * `ChatExchange` renders, and `deepLinkAnchorForEvent` trusts that claim to
   * decide whether an event addresses itself or needs its turn. There is no
   * jsdom here to render the component and read the real attributes, so this is
   * a source-scan, matching the `skeleton-guard` / `list-row-prose-guard`
   * precedent.
   *
   * If this fails you added a `data-event-id` to one of these components: add
   * the same id to `stampedEventIds`, then add its expression below.
   */
  const stampsIn = (path: string) => {
    const here = dirname(fileURLToPath(import.meta.url));
    const src = readFileSync(resolve(here, path), 'utf8');
    return [...src.matchAll(/data-event-id=\{([^}]*)\}/g)].map(m => m[1].trim());
  };

  it('matches every data-event-id ChatExchange actually stamps', () => {
    expect(stampsIn('../../../components/chat/ChatExchange.tsx')).toEqual([
      // The turn root: the exchange STARTER, and nothing for a turn with no
      // starter loaded (see `exchangeStarterId`).
      'starterId',
      // The failure card: the `ResponseFailed`'s own id (`exchangeError`).
      'error.eventId || undefined',
    ]);
  });

  it('matches every data-event-id an inline widget stamps', () => {
    // Its unavailable form takes the stamp it is handed; the widget hands it
    // only for an old `WidgetShown`, its own id.
    expect(stampsIn('../../../components/widgets/InlineWidget.tsx')).toEqual(['stamp', 'stamp']);
  });
});

/** Where opening a thread from Blocked lands. */
describe('landingTarget from Blocked', () => {
  function question(id: string, toolUseId: string, answered = false): Exchange {
    return {
      userEvent: evt('UserQuestionAsked', id, { tool_use_id: toolUseId }),
      userSeq: ++seq,
      steps: answered
        ? [{ seq: ++seq, event: evt('UserQuestionAnswered', `${id}-answer`, { tool_use_id: toolUseId }) }]
        : [],
    } as unknown as Exchange;
  }

  it('lands on the open question while the thread waits on the user', () => {
    const exchanges = [exchange('start-1'), question('q-1', 'tu-1'), exchange('start-2')];
    expect(landingTarget(exchanges, 'waiting_for_user_answer', 'blocked')).toEqual({ kind: 'card', eventId: 'q-1' });
  });

  it('skips an answered question for the open one', () => {
    const exchanges = [question('q-old', 'tu-old', true), question('q-new', 'tu-new')];
    expect(landingTarget(exchanges, 'waiting_for_user_answer', 'blocked')).toEqual({ kind: 'card', eventId: 'q-new' });
  });

  it('lands on the newest failure card when the thread failed', () => {
    const exchanges = [
      exchange('start-1', [['ResponseFailed', 'fail-old']]),
      exchange('start-2', [['ResponseFailed', 'fail-new']]),
    ];
    expect(landingTarget(exchanges, 'failed', 'blocked')).toEqual({ kind: 'card', eventId: 'fail-new' });
  });

  /** History must not win: an old unanswered question or failure card stays in
   *  the transcript after the thread moved on. */
  it('ignores a card that does not match the status, and lands on the newest turn', () => {
    const exchanges = [
      question('q-stale', 'tu-stale'),
      exchange('start-1', [['ResponseFailed', 'fail-old']]),
      exchange('start-2'),
    ];
    expect(landingTarget(exchanges, 'idle', 'blocked')).toEqual({ kind: 'turn', eventId: 'start-2' });
  });

  /** The card may be older than the loaded page, which the caller can fetch. */
  it('reports a card the loaded exchanges do not hold, with the newest turn as fallback', () => {
    expect(landingTarget([exchange('start-1'), exchange('start-2')], 'failed', 'blocked'))
      .toEqual({ kind: 'card-not-loaded', newestTurnId: 'start-2' });
  });

  it('carries no id when no turn has one', () => {
    const stray = exchange('unused');
    (stray.userEvent as { _eventId?: string })._eventId = undefined;
    expect(landingTarget([stray], 'idle', 'blocked')).toEqual({ kind: 'turn', eventId: null });
    expect(landingTarget([], 'failed', 'blocked')).toEqual({ kind: 'card-not-loaded', newestTurnId: null });
  });
});

/** Where opening a thread from Review lands (ADR 0409): the turn that proposed
 *  the ready change, else the newest turn, whose reply a read request names. */
describe('landingTarget from Review', () => {
  it('lands on the turn that proposed the change, not a later one', () => {
    const exchanges = [
      exchange('start-1', [['ChangeProposed', 'proposal', { change_id: 'c-1' }]]),
      exchange('start-2'),
    ];
    expect(landingTarget(exchanges, 'idle', 'review', true)).toEqual({ kind: 'turn', eventId: 'start-1' });
  });

  /** An applied or discarded change stays in its turn, so a read request on a
   *  later turn must not land on it. */
  it('lands on the newest turn when the old proposal is no longer ready', () => {
    const exchanges = [
      exchange('start-1', [['ChangeProposed', 'proposal', { change_id: 'c-1' }]]),
      exchange('start-2'),
    ];
    expect(landingTarget(exchanges, 'idle', 'review', false)).toEqual({ kind: 'turn', eventId: 'start-2' });
  });

  it('ignores a per-commit proposal, which carries no change id', () => {
    const exchanges = [
      exchange('start-1', [['ChangeProposed', 'commit', { change_id: '' }]]),
      exchange('start-2'),
    ];
    expect(landingTarget(exchanges, 'idle', 'review', true)).toEqual({ kind: 'turn', eventId: 'start-2' });
  });

  /** One thread in both groups: a failed run with a ready change. Each tile
   *  lands on its own target (plan invariant: target per group). */
  it('lands a failed thread with a ready change by the tile it was opened from', () => {
    const exchanges = [
      exchange('start-1', [['ChangeProposed', 'proposal', { change_id: 'c-1' }], ['ResponseFailed', 'fail']]),
    ];
    expect(landingTarget(exchanges, 'failed', 'blocked', true)).toEqual({ kind: 'card', eventId: 'fail' });
    expect(landingTarget(exchanges, 'failed', 'review', true)).toEqual({ kind: 'turn', eventId: 'start-1' });
  });

  it('lands on the newest turn for a read request, even on a failed thread', () => {
    const exchanges = [exchange('start-1', [['ResponseFailed', 'fail']]), exchange('start-2')];
    expect(landingTarget(exchanges, 'idle', 'review')).toEqual({ kind: 'turn', eventId: 'start-2' });
    expect(landingTarget(exchanges, 'failed', 'review')).toEqual({ kind: 'turn', eventId: 'start-2' });
  });
});


describe('readerHasReached', () => {
  const exchanges = [exchange('older'), exchange('target', [['ResponseFailed', 'failure']]), exchange('later', [['ResponseGenerated', 'later-reply']])];
  const target = landingTarget(exchanges, 'failed', 'blocked');

  it('compares a failure card by its turn, including a saved line in that turn', () => {
    expect(readerHasReached(exchanges, target, {
      kind: 'line', eventId: 'target', relTop: -40, turnRelTop: -200, path: [1], kinds: 'ffffffff',
    })).toBe(true);
  });

  it.each(['target', 'later'])('preserves an anchor in %s', eventId => {
    expect(readerHasReached(exchanges, target, { kind: 'anchor', eventId, relTop: 0 })).toBe(true);
  });

  it.each(['target', 'failure', 'later', 'later-reply'])('preserves a row owned by a reached turn: %s', rowEventId => {
    expect(readerHasReached(exchanges, target, { kind: 'row', rowEventId, relTop: -40 })).toBe(true);
  });

  it('preserves the live edge', () => {
    expect(readerHasReached(exchanges, target, { kind: 'live-edge' })).toBe(true);
  });

  it.each([null, { kind: 'offset' as const, top: 500 }, { kind: 'row' as const, rowEventId: 'older', relTop: 0 },
    { kind: 'row' as const, rowEventId: 'unloaded', relTop: 0 },
    { kind: 'anchor' as const, eventId: 'older', relTop: 0 },
    { kind: 'anchor' as const, eventId: 'unloaded', relTop: 0 }])('keeps landing for an unproven position %j', saved => {
    expect(readerHasReached(exchanges, target, saved)).toBe(false);
  });
});
