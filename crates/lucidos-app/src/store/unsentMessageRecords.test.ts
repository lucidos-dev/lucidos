import { describe, it, expect, beforeEach } from 'vitest';

import {
  adoptUnsentMessageRecord,
  createMemoryUnsentMessageBackend,
  forgetUnsentMessageRecord,
  markUnsentMessageRecordUnsent,
  persistSendingMessage,
  readUnsentMessageStore,
  unsentMessageDbName,
  unsentMessageStorageFailureMessage,
  _resetUnsentMessageRecordsForTesting,
  type UnsentMessageBackend,
  type UnsentMessageRecord,
} from './unsentMessageRecords';
import { pageOwnerId } from './pageOwner';
import { toasts } from './store';

function sending(eventId: string): Omit<UnsentMessageRecord, 'ownerId' | 'phase' | 'failedRetries'> {
  return {
    eventId,
    threadId: 't-1',
    body: { message: 'hello', mode: 'human', event_id: eventId, thread_id: 't-1' },
    settlement: { kind: 'follow-up' },
    sentAt: '2026-10-03T08:00:00.000Z',
  };
}

beforeEach(() => {
  _resetUnsentMessageRecordsForTesting();
  toasts.value = [];
});

describe('unsentMessageDbName', () => {
  it('gives each workspace its own database, since workspaces share one origin', () => {
    expect(unsentMessageDbName('alpha')).toBe('lucidos-unsent-messages:alpha');
    expect(unsentMessageDbName('beta')).not.toBe(unsentMessageDbName('alpha'));
    expect(unsentMessageDbName(null)).toBe('lucidos-unsent-messages');
  });
});

describe('the unsent message store', () => {
  it('keeps a send from the moment it starts, owned by this page', async () => {
    persistSendingMessage(sending('e-1'));
    const records = await readUnsentMessageStore();
    expect(records).toEqual([{ ...sending('e-1'), phase: 'sending', failedRetries: 0, ownerId: pageOwnerId }]);
  });

  it('marks a send that got no answer as unsent, with its retry count', async () => {
    persistSendingMessage(sending('e-1'));
    markUnsentMessageRecordUnsent('e-1', 2);
    const [record] = await readUnsentMessageStore();
    expect(record).toMatchObject({ phase: 'unsent', failedRetries: 2 });
  });

  it('runs writes in order, so a forget right after the put wins', async () => {
    persistSendingMessage(sending('e-1'));
    forgetUnsentMessageRecord('e-1');
    expect(await readUnsentMessageStore()).toEqual([]);
  });

  it('marking a record already forgotten brings nothing back', async () => {
    persistSendingMessage(sending('e-1'));
    forgetUnsentMessageRecord('e-1');
    markUnsentMessageRecordUnsent('e-1', 0);
    expect(await readUnsentMessageStore()).toEqual([]);
  });

  it('takes over a record an earlier page left', async () => {
    const earlier: UnsentMessageRecord = { ...sending('e-1'), phase: 'sending', failedRetries: 0, ownerId: 'an-earlier-page' };
    const seeded = createMemoryUnsentMessageBackend();
    await seeded.put(earlier);
    _resetUnsentMessageRecordsForTesting(seeded);
    adoptUnsentMessageRecord(earlier);
    const [record] = await readUnsentMessageStore();
    expect(record.ownerId).toBe(pageOwnerId);
  });

  it('says once that storage failed, and never throws into the send', async () => {
    const failing: UnsentMessageBackend = {
      ...createMemoryUnsentMessageBackend(),
      put: async () => { throw new Error('QuotaExceededError'); },
    };
    _resetUnsentMessageRecordsForTesting(failing);
    expect(() => {
      persistSendingMessage(sending('e-1'));
      persistSendingMessage(sending('e-2'));
    }).not.toThrow();
    await readUnsentMessageStore();
    const warnings = toasts.value.filter((t) => t.type === 'warning').map((t) => t.message);
    expect(warnings).toEqual([unsentMessageStorageFailureMessage(new Error('QuotaExceededError'))]);
  });
});
