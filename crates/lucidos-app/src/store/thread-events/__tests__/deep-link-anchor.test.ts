import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { landingTarget, deepLinkAnchorForEvent, stampedEventIds } from '../exchange-render';
import type { Exchange } from '../exchange';
import type { StoredEvent } from '../thread-event-types';

/**
 * A deep-link resolves in the DOM by `data-event-id`, and only three kinds of
 * element ever carry one. `deepLinkAnchorForEvent` is what lets a link to an event that
 * carries none of them (any ordinary step, e.g. the `CodingAgentIdled` an event wait
 * usually matches) still land, by re-targeting it at the turn that holds it.
 *
 * The set of self-stamping ids is declared once in `stampedEventIds`; the
 * tripwires here fail if `ChatExchange` or `WidgetCard` grows a stamp without
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

  /** "Show in thread" lands on the widget's card, not on the message before
   *  it, so the card's stamp must be declared. */
  it('returns the event itself for a WidgetShown, which stamps its own card', () => {
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

describe('stampedEventIds', () => {
  it('lists the starter, plus the failure card when the turn failed', () => {
    expect(stampedEventIds(exchange('start-1'))).toEqual(['start-1']);
    expect(stampedEventIds(exchange('start-1', [['ResponseFailed', 'fail-1']])))
      .toEqual(['start-1', 'fail-1']);
  });

  it('lists every widget card the turn showed', () => {
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
      'exchangeStarterId(exchange)',
      // The failure card: the `ResponseFailed`'s own id (`exchangeError`).
      'error.eventId || undefined',
    ]);
  });

  it('matches every data-event-id a widget card stamps', () => {
    // The card and its "no longer exists" form: the `WidgetShown`'s own id.
    expect(stampsIn('../../../components/widgets/WidgetCard.tsx')).toEqual(['eventId', 'eventId']);
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
