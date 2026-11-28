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

  it('returns the containing turn for a step that stamps nothing', () => {
    const exchanges = [
      exchange('start-1'),
      exchange('start-2', [['CodingAgentToolCalled', 'tool-1'], ['CodingAgentIdled', 'idle-1']]),
    ];
    expect(deepLinkAnchorForEvent(exchanges, 'idle-1')).toBe('start-2');
    expect(deepLinkAnchorForEvent(exchanges, 'tool-1')).toBe('start-2');
  });

  it('returns the event itself for a WidgetShown, which stamps its own inline widget', () => {
    const exchanges = [exchange('start-1', [['ToolCalled', 'tool-1'], ['WidgetShown', 'widget-1']])];
    expect(deepLinkAnchorForEvent(exchanges, 'widget-1')).toBe('widget-1');
  });

  it('returns null when no turn holds the event', () => {
    expect(deepLinkAnchorForEvent([exchange('start-1')], 'elsewhere')).toBeNull();
  });

  it('returns null for an event that merely landed in the open turn', () => {
    const exchanges = [
      exchange('question-1', [['BackgroundBashCompleted', 'bash-done-1']]),
    ];
    expect(deepLinkAnchorForEvent(exchanges, 'bash-done-1')).toBeNull();
  });

  it('keeps the containing turn for the bash START, which the turn caused', () => {
    const exchanges = [
      exchange('start-1', [['BackgroundBashStarted', 'bash-start-1']]),
    ];
    expect(deepLinkAnchorForEvent(exchanges, 'bash-start-1')).toBe('start-1');
  });

  it('still prefers a self-stamped element over the unanchorable rule', () => {
    const exchanges = [exchange('start-1', [['ResponseFailed', 'fail-1']])];
    expect(deepLinkAnchorForEvent(exchanges, 'fail-1')).toBe('fail-1');
  });

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

  it('names the turn holding an unanchorable event', () => {
    const exchanges = [exchange('start-1'), exchange('start-2', [['BackgroundBashCompleted', 'bash-1']])];
    expect(deepLinkTargetIndex(exchanges, { kind: 'event', eventId: 'bash-1' })).toBe(1);
  });

  it('answers -1 for a step of a continuation fragment', () => {
    const fragment = { ...exchange('frag-1', [['CodingAgentIdled', 'idle-2']]), continuationFragment: true as const };
    expect(deepLinkTargetIndex([fragment, exchange('start-2')], { kind: 'event', eventId: 'idle-2' })).toBe(-1);
  });

  it('answers -1 for an event no loaded turn holds', () => {
    expect(deepLinkTargetIndex([exchange('start-1')], { kind: 'event', eventId: 'older' })).toBe(-1);
  });

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

  const stampsIn = (path: string) => {
    const here = dirname(fileURLToPath(import.meta.url));
    const src = readFileSync(resolve(here, path), 'utf8');
    return [...src.matchAll(/data-event-id=\{([^}]*)\}/g)].map(m => m[1].trim());
  };

  it('matches every data-event-id ChatExchange actually stamps', () => {
    expect(stampsIn('../../../components/chat/ChatExchange.tsx')).toEqual([
      'starterId',
      'error.eventId || undefined',
    ]);
  });

  it('matches every data-event-id an inline widget stamps', () => {
    expect(stampsIn('../../../components/widgets/InlineWidget.tsx')).toEqual(['stamp', 'stamp']);
  });
});

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

  it('ignores a card that does not match the status, and lands on the newest turn', () => {
    const exchanges = [
      question('q-stale', 'tu-stale'),
      exchange('start-1', [['ResponseFailed', 'fail-old']]),
      exchange('start-2'),
    ];
    expect(landingTarget(exchanges, 'idle', 'blocked')).toEqual({ kind: 'turn', eventId: 'start-2' });
  });

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

describe('landingTarget from Review', () => {
  it('lands on the turn that proposed the change, not a later one', () => {
    const exchanges = [
      exchange('start-1', [['ChangeProposed', 'proposal', { change_id: 'c-1' }]]),
      exchange('start-2'),
    ];
    expect(landingTarget(exchanges, 'idle', 'review', true)).toEqual({ kind: 'turn', eventId: 'start-1' });
  });

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
