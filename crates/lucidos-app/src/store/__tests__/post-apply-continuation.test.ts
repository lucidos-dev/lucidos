import { describe, it, expect } from 'vitest';
import {
  groupIntoExchanges,
  exchangeResponseEvents,
  changePanelHasContinuation,
  type StoredEvent,
} from '../thread-events';

// Regression for real thread 76b4ee76 ("Queue Message Until Image Uploads"):
// a Claude Code session applied a change, then KEPT WORKING (no new user
// message) and proposed a second change. In the timeline the two "Change
// applied" banners rendered back-to-back with nothing between them — the whole
// second change (its work + the proposal) was invisible. Two causes:
//   1. The post-apply turn's context snapshots (ContextCaptured) carry the CC
//      session's persistent request_event_id (anchored to the original
//      message), so they were request-id routed BACK to the first message's
//      exchange — out from between the change banners.
//   2. The post-apply CC reply folds into the ChangeApplied exchange as a step,
//      but a change-lifecycle banner suppresses its response body, so the reply
//      never rendered.
describe('post-apply CC continuation is visible between change banners', () => {
  const cc = 'claude_code';
  const evt = (o: Record<string, unknown>): StoredEvent => o as StoredEvent;

  // Mirrors the real event stream: one message, a first change applied while
  // the session stays alive, then a second turn (same session, same req id)
  // with no new MessageReceived.
  const events = (): Map<number, StoredEvent> =>
    new Map<number, StoredEvent>([
      [1, evt({ type: 'MessageReceived', text: 'queue messages', channel: cc, _eventId: 'mr1', created: '2026-06-14T08:30:46Z' })],
      [2, evt({ type: 'CodingAgentToolCalled', name: 'Edit', args: {}, tool_use_id: 't1', channel: cc, request_event_id: 'mr1', created: '2026-06-14T08:31:10Z' })],
      [3, evt({ type: 'CodingAgentToolResult', tool_use_id: 't1', result: 'ok', channel: cc, request_event_id: 'mr1', created: '2026-06-14T08:31:12Z' })],
      [4, evt({ type: 'CodingAgentIdled', created: '2026-06-14T08:34:15Z' })],
      [5, evt({ type: 'ChangeProposed', change_id: 'A', files: ['a.ts', 'b.ts'], created: '2026-06-14T08:34:16Z' })],
      [6, evt({ type: 'ChangeApplied', change_id: 'A', _eventId: 'caA', created: '2026-06-14T08:34:35Z' })],
      // ── post-apply turn: no new MessageReceived; reuses session req id mr1 ──
      [7, evt({ type: 'ContextCaptured', producer: 'coding_agent', model: 'x', context_window: 200000, estimated_total_tokens: 1, sections: [], channel: cc, request_event_id: 'mr1', created: '2026-06-14T08:36:00Z' })],
      [8, evt({ type: 'CodingAgentTextStreamed', text: 'Working on change B', channel: cc, request_event_id: 'mr1', created: '2026-06-14T08:38:15Z' })],
      [9, evt({ type: 'CodingAgentIdled', created: '2026-06-14T08:38:16Z' })],
      [10, evt({ type: 'ChangeProposed', change_id: 'B', files: ['c.ts', 'd.ts', 'e.ts', 'f.ts'], created: '2026-06-14T08:38:17Z' })],
      [11, evt({ type: 'ChangeApplied', change_id: 'B', _eventId: 'caB', created: '2026-06-14T09:32:23Z' })],
    ]);

  const applyAFrom = (map: Map<number, StoredEvent>) => {
    const exchanges = groupIntoExchanges(map);
    const applyA = exchanges.find(
      e => e.userEvent.type === 'ChangeApplied' && (e.userEvent as { change_id?: string }).change_id === 'A',
    );
    expect(applyA).toBeTruthy();
    return applyA!;
  };

  it('keeps post-apply context snapshots with the apply turn, not the original message', () => {
    const applyA = applyAFrom(events());
    const seqs = applyA.steps.map(s => s.seq);
    // Cause #1: the post-apply ContextCaptured (seq 7) must fold into the apply
    // turn chronologically — NOT route by request_event_id back to mr1.
    expect(seqs).toContain(7);
  });

  it('surfaces the post-apply CC reply so the banner renders a body', () => {
    const applyA = applyAFrom(events());
    // Cause #2: the change banner must be flagged as carrying continuation work…
    expect(changePanelHasContinuation(applyA)).toBe(true);
    // …and its response events must include the reply text.
    const rendered = exchangeResponseEvents(applyA, false, true);
    const text = rendered
      .filter(e => e.type === 'text')
      .map(e => (e as { md: string }).md)
      .join(' ');
    expect(text).toContain('Working on change B');
  });

  it('does not flag a plain applied change (no continuation) as having a body', () => {
    // A normal apply with no trailing CC work must stay body-less so the common
    // case still renders as just the banner.
    const map = new Map<number, StoredEvent>([
      [1, evt({ type: 'MessageReceived', text: 'go', channel: cc, _eventId: 'mr1', created: '2026-06-14T08:00:00Z' })],
      [2, evt({ type: 'CodingAgentIdled', created: '2026-06-14T08:01:00Z' })],
      [3, evt({ type: 'ChangeProposed', change_id: 'A', files: ['a.ts'], created: '2026-06-14T08:01:01Z' })],
      [4, evt({ type: 'ChangeApplied', change_id: 'A', _eventId: 'caA', created: '2026-06-14T08:02:00Z' })],
    ]);
    const exchanges = groupIntoExchanges(map);
    const applyA = exchanges.find(e => e.userEvent.type === 'ChangeApplied')!;
    expect(changePanelHasContinuation(applyA)).toBe(false);
  });
});
