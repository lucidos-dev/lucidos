import { describe, it, expect } from 'vitest';
import { TS, makeThreadState } from './thread-events-helpers';
import { exchangeResponseEvents, groupIntoExchanges, type StoredEvent, type ThreadEvent } from '../thread-events';
import { checkpointUndoScope } from '../../components/chat/chat-exchange-parts';

function checkpointRow(events: ThreadEvent[]) {
  const thread = makeThreadState();
  events.forEach((event, i) => {
    thread.events.set(i + 1, { ...event, created: TS, id: `e${i + 1}` } as unknown as StoredEvent);
  });
  const rows = groupIntoExchanges(thread.events).flatMap(x => exchangeResponseEvents(x));
  return rows.find(r => r.type === 'checkpoint');
}

const MESSAGE: ThreadEvent = {
  type: 'MessageReceived',
  text: 'clean up the staging dir',
} as unknown as ThreadEvent;

describe('checkpoint card rendering', () => {
  it('carries the engine\'s counts onto the row', () => {
    const row = checkpointRow([
      MESSAGE,
      {
        type: 'CommandCheckpointed',
        checkpoint_id: 'c1',
        command: 'rm -rf data/tmp',
        summary: 'Deletes files inside the workspace.',
        restores: 3,
        removes: 1,
      } as unknown as ThreadEvent,
    ]);
    expect(row).toMatchObject({ type: 'checkpoint', restores: 3, removes: 1, reverted: false });
  });

  it('defaults the counts on a checkpoint written before they existed', () => {
    const row = checkpointRow([
      MESSAGE,
      {
        type: 'CommandCheckpointed',
        checkpoint_id: 'legacy',
        command: 'rm -rf data/tmp',
        summary: 'Deletes files inside the workspace.',
      } as unknown as ThreadEvent,
    ]);
    expect(row).toMatchObject({ type: 'checkpoint', restores: 0, removes: 0 });
  });

  it('flips to reverted when the paired revert lands in the exchange', () => {
    const row = checkpointRow([
      MESSAGE,
      {
        type: 'CommandCheckpointed',
        checkpoint_id: 'c1',
        command: 'rm -rf data/tmp',
        summary: 'Deletes files inside the workspace.',
        restores: 1,
        removes: 1,
      } as unknown as ThreadEvent,
      { type: 'CommandCheckpointReverted', checkpoint_id: 'c1' } as unknown as ThreadEvent,
    ]);
    expect(row).toMatchObject({ reverted: true });
  });
});

describe('checkpointUndoScope', () => {
  it('names both halves of what Undo does', () => {
    expect(checkpointUndoScope(2, 1)).toBe(
      'Undo will restore 2 files and remove 1 file this step created.',
    );
  });

  it('names only the half that applies', () => {
    expect(checkpointUndoScope(3, 0)).toBe('Undo will restore 3 files.');
    expect(checkpointUndoScope(0, 1)).toBe('Undo will remove 1 file this step created.');
  });

  it('says nothing when the counts are unknown', () => {
    expect(checkpointUndoScope(0, 0)).toBeNull();
  });
});
