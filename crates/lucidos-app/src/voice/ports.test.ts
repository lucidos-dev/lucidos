import { describe, it, expect, vi } from 'vitest';
import { ApiError } from '../api/client';
import { browserPorts } from './ports';
import { CallSetupError, NO_ROUTE_FOR_A_CALL, setupRefusal } from './refusals';

let answer: (threadId: string) => Promise<string> = () => Promise.resolve('');
vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>();
  return { ...actual, mintCallTicket: (threadId: string) => answer(threadId) };
});

/** What the reader sees when a mint fails, which is the point of minting. */
async function refusal(): Promise<string> {
  try {
    await browserPorts.mintCallTicket('thread-1');
  } catch (err) {
    return setupRefusal(err);
  }
  throw new Error('the mint did not refuse');
}

describe('the ticket mint', () => {
  it('hands back the ticket for the thread asked about', async () => {
    answer = (threadId) => Promise.resolve(`t-${threadId}`);
    await expect(browserPorts.mintCallTicket('thread-1')).resolves.toBe('t-thread-1');
  });

  it('shows an engine refusal as the engine wrote it', async () => {
    const reason = 'This thread is already on a call.';
    answer = () => Promise.reject(new ApiError(409, reason));
    await expect(browserPorts.mintCallTicket('thread-1')).rejects.toBeInstanceOf(CallSetupError);
    expect(await refusal()).toBe(reason);
  });

  it('names the route when the engine cannot be reached', async () => {
    answer = () => Promise.reject(new TypeError('Failed to fetch'));
    expect(await refusal()).toBe(NO_ROUTE_FOR_A_CALL);
  });
});
