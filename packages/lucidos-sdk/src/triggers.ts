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

/** An irreversible-side-effect category a trigger can be granted permission to
 *  perform unattended. Only meaningful when the workspace's command guard is on
 *  (Settings → Permissions → Command Safety). A trigger that hits an
 *  irreversible command whose category isn't in its grant is failed. */
export type SideEffectCategory =
  | 'email'
  | 'external_api'
  | 'cloud_cli'
  | 'out_of_workspace_destruction'
  | 'other';

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
  /** Side-effect grant — irreversible categories this trigger may perform
   *  unattended. Omitted when empty (= no grant). */
  side_effect_grant?: SideEffectCategory[];
  /** Chat model this trigger's intent fires on. Omitted when the trigger uses
   *  the account default (Settings → Models → Chat & triggers). Intent
   *  triggers only: a script trigger runs no LLM. */
  model?: string;
  /** Thinking budget for this trigger's intent fires, one of
   *  `none|low|medium|high|xhigh|max`. Omitted when the trigger uses the
   *  account default. */
  reasoning_effort?: string;
}

export interface CreateTrigger {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  /** Optional *trigger group* id (UUID string). Pure organizational label —
   *  the trigger fires identically regardless of group. Omit for ungrouped. */
  group_id?: string;
  /** Side-effect grant — irreversible categories this trigger may perform
   *  unattended. Omit / `[]` = none granted (the safe default). */
  side_effect_grant?: SideEffectCategory[];
  /** Chat model this trigger's intent fires on. Omit / null = the account
   *  default. Not validated against the model registry, so a wrong id fails at
   *  fire time rather than at save time. */
  model?: string | null;
  /** Thinking budget for this trigger's intent fires. Omit / null = the account
   *  default. Must be one of `none|low|medium|high|xhigh|max`. */
  reasoning_effort?: string | null;
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
  /** Full replacement for the side-effect grant. Send the complete new set;
   *  pass `[]` to clear all grants. */
  side_effect_grant?: SideEffectCategory[];
  /** Pin the trigger's intent to a chat model (string) or clear it back to the
   *  account default (null). Absent leaves it unchanged. */
  model?: string | null;
  /** Pin the trigger's thinking budget (`none|low|medium|high|xhigh|max`) or
   *  clear it back to the account default (null). Absent leaves it unchanged. */
  reasoning_effort?: string | null;
}

export interface ApiResult {
  success: boolean;
  error?: string;
}

/** Result of an off-schedule run. `success: true` with
 *  `status: 'already-running'` means the request was valid and nothing new
 *  started: a fire of this trigger was already active or queued, and scheduled
 *  fires coalesce to at most one pending run per trigger. Never present that
 *  as a started run. */
export interface TriggerRunResult {
  success: boolean;
  status?: 'started' | 'queued' | 'already-running';
  /** Human-readable summary; on a refusal this is the reason. */
  message: string;
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

  /** Fire an existing trigger once, right now, outside its schedule.
   *
   *  This is a real fire, not an imitation: it records `TriggerExecuted` and
   *  `last_run`, and runs under the trigger's own identity, side-effect grant
   *  and `go_to_review` routing. Resolves as soon as the run is admitted, not
   *  when it finishes.
   *
   *  Refused (`success: false`) when the trigger is paused and when it has no
   *  cron schedule (emit its subscribed event instead).
   *
   *  The engine also refuses a run requested from inside a trigger fire, but
   *  that guard reads a task-local set only on the in-process LLM-tool path, so
   *  it does NOT fire for this HTTP call. Asking a trigger to run itself from
   *  its own script is bounded anyway: the trigger is active, so the fire
   *  coalesces and comes back as `already-running`. */
  run(id: string): Promise<TriggerRunResult> {
    return request(`/triggers/run?id=${encodeURIComponent(id)}`, {
      method: 'POST',
    });
  },
};
