import { API, json, mutatingFetch, throwIfNotOk } from './_core';
import { lucidos } from '@lucidos/sdk';
import type { HistoricalTriggerInfo, TriggerGroup } from '../../store/types';
import type { ApiResult, TriggerRunResult, TriggersListResponse } from '../types';

export function listTriggers(): Promise<TriggersListResponse> {
  return lucidos.triggers.list().then(triggers => ({ triggers })) as Promise<TriggersListResponse>;
}

type TriggerFormFields = {
  go_to_review?: boolean;
  revoke_program_denies?: string[];
  revoke_folder_denies?: string[];
};

export function createTrigger(
  body: Parameters<typeof lucidos.triggers.create>[0] & TriggerFormFields,
): Promise<ApiResult> {
  return lucidos.triggers.create(body) as Promise<ApiResult>;
}

export function updateTrigger(
  id: string,
  body: Parameters<typeof lucidos.triggers.update>[1] & TriggerFormFields,
): Promise<ApiResult> {
  return lucidos.triggers.update(id, body) as Promise<ApiResult>;
}

export function deleteTriggerApi(
  id: string
): Promise<ApiResult> {
  return lucidos.triggers.delete(id) as Promise<ApiResult>;
}

export function runTriggerApi(id: string): Promise<TriggerRunResult> {
  return lucidos.triggers.run(id) as Promise<TriggerRunResult>;
}

export function listHistoricalTriggers(): Promise<{ triggers: HistoricalTriggerInfo[] }> {
  return json(`${API}/triggers/historical`);
}


export function listTriggerGroups(): Promise<{ groups: TriggerGroup[] }> {
  return json(`${API}/trigger-groups`);
}

export function createTriggerGroup(body: { name: string; order?: number }): Promise<TriggerGroup> {
  return json(`${API}/trigger-groups`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export function updateTriggerGroup(
  id: string,
  body: { name?: string; order?: number },
): Promise<TriggerGroup> {
  return json(`${API}/trigger-groups?id=${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export interface TriggerGroupDeleteError {
  error: 'non_empty';
  member_count: number;
  member_trigger_ids: string[];
}

export async function deleteTriggerGroup(id: string): Promise<TriggerGroupDeleteError | null> {
  const res = await mutatingFetch(`${API}/trigger-groups?id=${encodeURIComponent(id)}`, { method: 'DELETE' });
  if (res.status === 204) return null;
  if (res.status === 409) {
    const body = await res.json() as TriggerGroupDeleteError;
    return body;
  }
  await throwIfNotOk(res);
  return null;
}

export async function reorderTriggerGroups(
  ordering: Array<{ id: string; order: number }>,
): Promise<void> {
  const res = await mutatingFetch(`${API}/trigger-groups/reorder`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ordering }),
  });
  await throwIfNotOk(res);
}
