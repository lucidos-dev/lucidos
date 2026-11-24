import { request } from './_fetch';
import { assertPlainObject, assertArray } from './_validate';

export type TriggerRun =
  | { type: 'intent'; intent: string }
  | { type: 'script'; path: string };

export interface EventSubscription {
  event_type: string;
  condition?: Record<string, unknown>;
}

export type SideEffectCategory =
  | 'email'
  | 'external_api'
  | 'cloud_cli'
  | 'out_of_workspace_destruction'
  | 'other';

export interface Trigger {
  id: string;
  name: string;
  slug: string;
  cron_expressions: string[];
  timezone: string;
  paused: boolean;
  last_run?: string;
  next_run?: string;
  run: TriggerRun;
  on?: EventSubscription[];
  side_effect_grant?: SideEffectCategory[];
  program_grants?: { pattern: string; migrated_invocation: boolean }[];
  folder_grants?: { folder: string }[];
  coding_agent_grant: boolean;
  mcp_servers?: string[];
  model?: string;
  reasoning_effort?: string;
  provider?: string;
  continue_key?: ContinueKeySpec;
  actor?: string;
}

export type ContinueKeySpec = (string | string[])[];

export interface CreateTrigger {
  name: string;
  slug?: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  group_id?: string;
  coding_agent_grant?: boolean;
  mcp_servers?: string[];
  model?: string | null;
  reasoning_effort?: string | null;
  provider?: string | null;
  continue_key?: ContinueKeySpec | null;
  actor?: string | null;
}

export interface UpdateTrigger {
  name?: string;
  run?: TriggerRun;
  cron_expressions?: string[];
  paused?: boolean;
  on?: EventSubscription[];
  group_id?: string | null;
  side_effect_grant?: [];
  revoke_program_grants?: string[];
  revoke_folder_grants?: string[];
  coding_agent_grant?: boolean;
  mcp_servers?: string[];
  model?: string | null;
  reasoning_effort?: string | null;
  provider?: string | null;
  continue_key?: ContinueKeySpec | null;
  actor?: string | null;
}

export interface ApiResult {
  success: boolean;
  error?: string;
}

export interface TriggerRunResult {
  success: boolean;
  status?: 'started' | 'queued' | 'already-running';
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

  run(id: string): Promise<TriggerRunResult> {
    return request(`/triggers/run?id=${encodeURIComponent(id)}`, {
      method: 'POST',
    });
  },
};
