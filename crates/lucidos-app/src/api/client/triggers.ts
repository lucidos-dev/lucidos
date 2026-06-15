import { API, json, mutatingFetch, throwIfNotOk } from './_core';
import { lucidos } from '@lucidos/sdk';
import type {
  EventSubscription,
  HistoricalTriggerInfo,
  SideEffectCategory,
  TriggerGroup,
  TriggerRun,
} from '../../store/types';
import type { ApiResult, TriggersListResponse } from '../types';

// --- Triggers (SDK delegation) ---
export function listTriggers(): Promise<TriggersListResponse> {
  return lucidos.triggers.list().then(triggers => ({ triggers })) as Promise<TriggersListResponse>;
}

export function createTrigger(body: {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  go_to_review?: boolean;
  /** Optional *trigger group* id (UUID string). Omit for ungrouped. */
  group_id?: string;
  /** Side-effect grant (ADR 0002, Phase 5) — irreversible categories this
   *  trigger may perform unattended. Omit/[] = none granted. */
  side_effect_grant?: SideEffectCategory[];
}): Promise<ApiResult> {
  return lucidos.triggers.create(body) as Promise<ApiResult>;
}

export function updateTrigger(
  id: string,
  body: {
    name?: string;
    run?: TriggerRun;
    cron_expressions?: string[];
    paused?: boolean;
    /** Full replacement; send [] to clear all subscriptions. */
    on?: EventSubscription[];
    go_to_review?: boolean;
    /** Move into a *trigger group* (string id) or out of any group (null).
     *  Absent leaves membership unchanged. */
    group_id?: string | null;
    /** Full replacement for the side-effect grant; send [] to clear all. */
    side_effect_grant?: SideEffectCategory[];
  }
): Promise<ApiResult> {
  return lucidos.triggers.update(id, body) as Promise<ApiResult>;
}

export function deleteTriggerApi(
  id: string
): Promise<ApiResult> {
  return lucidos.triggers.delete(id) as Promise<ApiResult>;
}

// Bypasses the @lucidos/sdk delegation above — this is engine-internal
// (powers the thread-filter dropdown), not part of the public app API.
export function listHistoricalTriggers(): Promise<{ triggers: HistoricalTriggerInfo[] }> {
  return json(`${API}/triggers/historical`);
}

// --- Trigger groups (engine-internal; the SDK doesn't expose grouping today) ---

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

/** Returns the 409 body on non-empty delete, lets caller surface the members. */
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
