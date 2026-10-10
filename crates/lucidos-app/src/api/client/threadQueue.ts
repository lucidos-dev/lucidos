import { API, json, mutatingFetch, throwIfNotOk } from './_core';
import type { CapacityPolicy, ThreadQueueResponse } from '../../store/types';

// --- Thread Queue (admission control for background spawns) ---

export function getThreadQueue(): Promise<ThreadQueueResponse> {
  return json<ThreadQueueResponse>(`${API}/thread-queue`);
}

/** Force-admit a queued entry, ignoring every capacity cap. */
export async function runThreadQueueEntryNow(entryId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/thread-queue/run-now`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ entry_id: entryId }),
  });
  await throwIfNotOk(res);
}

/** Drop a queued entry without running it. */
export async function dropThreadQueueEntry(entryId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/thread-queue/drop`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ entry_id: entryId }),
  });
  await throwIfNotOk(res);
}

/** Replace the capacity policy. Returns the policy as the engine stored it. */
export async function putCapacityPolicy(policy: CapacityPolicy): Promise<CapacityPolicy> {
  const res = await mutatingFetch(`${API}/thread-queue/policy`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(policy),
  });
  await throwIfNotOk(res);
  return res.json();
}
