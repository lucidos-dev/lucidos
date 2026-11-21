/** An effort pick mid-turn is one turn, not two.
 *
 *  Claude Code takes effort only at spawn. So the engine cancels the turn as a
 *  redirect (`effort_restart`), idles the session, and resumes it at the new
 *  effort under a `ContinuationStarted`. That resume used to open a turn of its
 *  own. The interrupted turn then read "Done" under its own resume card, which
 *  read as the explanation of the effort change.
 *
 *  Now the resume folds into the turn it interrupted, as a restart row. The
 *  turn keeps reading Working through the gap before the resume lands.
 */
import { describe, expect, it } from 'vitest';
import { exchangeResponseEvents, exchangeStatus, groupIntoExchanges, type StoredEvent } from '../thread-events';

const DEVICE = { kind: 'device', device_id: 'my-mac' } as const;

/** The reported order: a turn working, the effort pick, the resume. */
function restartedThread(until: 'canceled' | 'idled' | 'resumed' | 'working'): Map<number, StoredEvent> {
  const events: [number, StoredEvent][] = [
    [1, { type: 'MessageReceived', text: 'run the suites', mode: 'human', _eventId: 'msg-1' }],
    [2, { type: 'SessionStarted', channel: 'claude_code' } as StoredEvent],
    [3, { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, tool_use_id: 'tu-1' } as StoredEvent],
    [4, { type: 'ResponseCanceled', cause: 'effort_restart', actor: DEVICE, reasoning_effort: 'medium' } as StoredEvent],
    [5, { type: 'CodingAgentIdled' } as StoredEvent],
    [6, { type: 'ContinuationRequested', reason: 'effort_restart', actor: DEVICE } as StoredEvent],
    [7, { type: 'ContinuationStarted', reason: 'effort_restart', actor: DEVICE, _eventId: 'resume-1' } as StoredEvent],
    [8, { type: 'SessionStarted', channel: 'claude_code' } as StoredEvent],
    [9, { type: 'CodingAgentToolCalled', name: 'Bash', args: {}, tool_use_id: 'tu-2' } as StoredEvent],
  ];
  const last = { canceled: 4, idled: 5, resumed: 8, working: 9 }[until];
  return new Map(events.filter(([seq]) => seq <= last));
}

describe('an effort restart is one turn', () => {
  it('folds the resume into the turn it interrupted', () => {
    const exchanges = groupIntoExchanges(restartedThread('working'));
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived']);
    const steps = exchanges[0].steps.map(s => s.event.type);
    expect(steps).toContain('ContinuationStarted');
    expect(steps[steps.length - 1]).toBe('CodingAgentToolCalled');
  });

  it('draws the resume as a restart row where it happened', () => {
    const [turn] = groupIntoExchanges(restartedThread('working'));
    const rows = exchangeResponseEvents(turn, false, true);
    const restart = rows.findIndex(r => r.type === 'restart');
    expect(restart).toBeGreaterThan(-1);
    expect(rows[restart]).toMatchObject({ type: 'restart', reason: 'effort_restart' });
    expect(rows.slice(restart + 1).some(r => r.type === 'step')).toBe(true);
  });

  it.each(['canceled', 'idled'] as const)('keeps reading Working while the resume is owed (%s)', (until) => {
    const [turn] = groupIntoExchanges(restartedThread(until));
    expect(exchangeStatus(turn, '', true, false, true, true)).toBe('coding-agent-working');
  });

  it('reads Working once the resumed session runs', () => {
    const [turn] = groupIntoExchanges(restartedThread('resumed'));
    expect(exchangeStatus(turn, '', true, false, true, false)).toBe('coding-agent-working');
  });

  it('reads Done when the resumed session idles', () => {
    const events = restartedThread('working');
    events.set(10, { type: 'CodingAgentIdled' } as StoredEvent);
    const [turn] = groupIntoExchanges(events);
    expect(exchangeStatus(turn, '', true, false, true, true)).toBe('done');
  });

  // A follow-up drives the thread instead, and the engine owes no resume.
  it('owes nothing once a follow-up takes over', () => {
    const events = restartedThread('idled');
    events.set(6, { type: 'MessageReceived', text: 'do this instead', mode: 'human', _eventId: 'msg-2' });
    const exchanges = groupIntoExchanges(events);
    expect(exchangeStatus(exchanges[0], '', false, false, true, true)).toBe('done');
  });

  // The cancel and idle before the resume ended the replaced run, not the
  // turn. A message sent now waits for the resumed agent to read it.
  it('queues a follow-up behind the resumed turn', () => {
    const events = restartedThread('working');
    events.set(2, { type: 'CodingAgentInputRead', input_event_id: 'msg-1' } as StoredEvent);
    events.set(10, { type: 'MessageReceived', text: 'also check the totals', mode: 'human', channel: 'claude_code', _eventId: 'msg-2' } as StoredEvent);
    events.set(11, { type: 'CodingAgentTextStreamed', text: 'still on the suites', channel: 'claude_code' } as StoredEvent);
    const [turn, followUp] = groupIntoExchanges(events);
    expect(followUp.awaitingRead).toBe(true);
    expect(followUp.steps).toEqual([]);
    expect(turn.steps[turn.steps.length - 1].event.type).toBe('CodingAgentTextStreamed');
  });

  // An abort after the cancel ends the turn some other way. A resume that
  // still lands draws below the abort, where it happened.
  it('owes nothing once an abort ends the turn', () => {
    const events = restartedThread('working');
    events.set(6, { type: 'ResponseAborted', cause: 'engine_shutdown', actor: DEVICE } as StoredEvent);
    const types = groupIntoExchanges(events).map(e => e.userEvent.type);
    expect(types).toEqual(['MessageReceived', 'ResponseAborted', 'ContinuationStarted']);
  });

  // A child's report holds no turn, so the resume still folds in.
  it('folds the resume past a turnless boundary', () => {
    const events = restartedThread('working');
    events.set(6, { type: 'ChildReportFiled', child_thread_id: 'child-1', summary: 'done' } as StoredEvent);
    const types = groupIntoExchanges(events).map(e => e.userEvent.type);
    expect(types).not.toContain('ContinuationStarted');
  });

  // Any other resume still opens its own turn.
  it('leaves a resume for another reason as its own turn', () => {
    const events = restartedThread('working');
    events.set(4, { type: 'ResponseAborted', cause: 'engine_shutdown', actor: DEVICE } as StoredEvent);
    events.set(7, { type: 'ContinuationStarted', reason: 'auto_resume_after_switch', _eventId: 'resume-1' } as StoredEvent);
    const types = groupIntoExchanges(events).map(e => e.userEvent.type);
    expect(types[types.length - 1]).toBe('ContinuationStarted');
  });
});
