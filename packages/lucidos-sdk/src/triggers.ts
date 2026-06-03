import { request } from './_fetch';
import { assertPlainObject, assertArray } from './_validate';

export type TriggerRun =
  | { type: 'intent'; intent: string }
  | { type: 'script'; path: string };

/** One event a trigger listens for, with an optional payload filter scoped to
 *  that event. A trigger fires when an incoming event matches any entry's
 *  `event_type` AND that entry's `condition` (if set) evaluates true against
 *  the payload. Conditions are per-entry, so a single trigger can subscribe
 *  to events with different payload shapes without one filter constraining
 *  the others. */
export interface EventSubscription {
  event_type: string;
  condition?: Record<string, unknown>;
}

export interface Trigger {
  id: string;
  name: string;
  cron_expressions: string[];
  timezone: string;
  paused: boolean;
  last_run?: string;
  next_run?: string;
  run: TriggerRun;
  /** Event subscriptions. Empty for schedule-only triggers; the engine
   *  omits the field rather than emitting `[]`, so readers must tolerate
   *  absence. */
  on?: EventSubscription[];
}

export interface CreateTrigger {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  /** Optional *trigger group* id (UUID string). Pure organizational label —
   *  the trigger fires identically regardless of group. Omit for ungrouped. */
  group_id?: string;
}

export interface UpdateTrigger {
  name?: string;
  run?: TriggerRun;
  cron_expressions?: string[];
  paused?: boolean;
  /** Full replacement for the event subscription list. Send the complete new
   *  set — there is no partial edit. Pass `[]` to clear all subscriptions. */
  on?: EventSubscription[];
  /** Move the trigger into a *trigger group* (string id) or out of any group
   *  (null). Absent leaves membership unchanged. */
  group_id?: string | null;
}

export interface ApiResult {
  success: boolean;
  error?: string;
}

export const triggers = {
  list(): Promise<Trigger[]> {
    return request<{ triggers: Trigger[] }>('/triggers').then(r => r.triggers);
  },

  create(trigger: CreateTrigger): Promise<ApiResult> {
    assertPlainObject('trigger', trigger);
    assertArray('trigger.cron_expressions', trigger.cron_expressions);
    return request('/triggers', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(trigger),
    });
  },

  update(id: string, trigger: UpdateTrigger): Promise<ApiResult> {
    assertPlainObject('trigger', trigger);
    if (trigger.cron_expressions !== undefined) {
      assertArray('trigger.cron_expressions', trigger.cron_expressions);
    }
    return request(`/triggers?id=${encodeURIComponent(id)}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(trigger),
    });
  },

  delete(id: string): Promise<ApiResult> {
    return request(`/triggers?id=${encodeURIComponent(id)}`, {
      method: 'DELETE',
    });
  },
};
