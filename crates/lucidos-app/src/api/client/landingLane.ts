/**
 * The *landing lane*'s and the *main check*'s HTTP actions (ADRs 0463, 0464).
 *
 * Not in the `api/client` barrel: only the lazy lane surfaces call these, so
 * keeping them out of it keeps them off the first paint (ADR 0454).
 */

import { API, json } from './_core';

/** Take a change waiting in the landing lane back out. It stays pending.
 *  A member of the open lane batch is refused with 409: its batch goes whole. */
export async function removeFromLandingLane(changeId: string): Promise<{ message: string }> {
  return json(`${API}/changes/${encodeURIComponent(changeId)}/queue`, { method: 'DELETE' });
}

/** Cancel the open lane batch: it fails whole, its lane thread stops, and
 *  every member stays pending. `LaneBatchFailed` follows on the stream. */
export async function cancelLaneBatch(batchId: string): Promise<{ canceled: string }> {
  return json(`${API}/changes/lane/cancel`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ batch_id: batchId }),
  });
}

/** Close the red main check, as the maintainer. Its fix thread stays an
 *  ordinary thread. 409 when main is not red. */
export async function closeMainCheckRed(): Promise<{ message: string }> {
  return json(`${API}/main-check/close`, { method: 'POST' });
}
