import { request } from './_fetch';
import { assertPlainObject, assertArray } from './_validate';

export type TriggerRun =
  | { type: 'intent'; intent: string; knowhow: string[] }
  | { type: 'script'; path: string };

export interface Trigger {
  id: string;
  name: string;
  cron_expressions: string[];
  timezone: string;
  enabled: boolean;
  last_run?: string;
  next_run?: string;
  run: TriggerRun;
  on?: string;
  condition?: Record<string, unknown>;
}

export interface CreateTrigger {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on_event?: string;
  condition?: Record<string, unknown>;
}

export interface UpdateTrigger {
  name?: string;
  run?: TriggerRun;
  cron_expressions?: string[];
  enabled?: boolean;
  on_event?: string | null;
  condition?: Record<string, unknown> | null;
}

export interface ApiResult {
  success: boolean;
  error?: string;
}

export const triggers = {
  list(): Promise<Trigger[]> {
    return request<{ triggers: Trigger[] }>('/api/triggers').then(r => r.triggers);
  },

  create(trigger: CreateTrigger): Promise<ApiResult> {
    assertPlainObject('trigger', trigger);
    assertArray('trigger.cron_expressions', trigger.cron_expressions);
    return request('/api/triggers', {
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
    return request(`/api/triggers?id=${encodeURIComponent(id)}`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(trigger),
    });
  },

  delete(id: string): Promise<ApiResult> {
    return request(`/api/triggers?id=${encodeURIComponent(id)}`, {
      method: 'DELETE',
    });
  },
};
