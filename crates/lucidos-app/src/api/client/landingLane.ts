
import { API, json } from './_core';

export async function removeFromLandingLane(changeId: string): Promise<{ message: string }> {
  return json(`${API}/changes/${encodeURIComponent(changeId)}/queue`, { method: 'DELETE' });
}

export async function cancelLaneBatch(batchId: string): Promise<{ canceled: string }> {
  return json(`${API}/changes/lane/cancel`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ batch_id: batchId }),
  });
}

export async function closeMainCheckRed(): Promise<{ message: string }> {
  return json(`${API}/main-check/close`, { method: 'POST' });
}

export async function fixMainNow(): Promise<{ message: string }> {
  return json(`${API}/main-check/fix-now`, { method: 'POST' });
}
