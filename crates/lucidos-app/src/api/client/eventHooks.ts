import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export interface EventHookDefinition {
  name: string;
  event_type: string;
  condition?: unknown;
  script: string;
  timeout_secs?: number;
}

export type EventHookArmState = 'disarmed' | 'armed' | 'changed';

export interface EventHookArming {
  slug: string;
  event_type: string;
  content_hash: string;
  armed_at: string;
}

export type EventHook = {
  arm_state: EventHookArmState;
  armed: EventHookArming | null;
  arm_refusal: string | null;
  remove_refusal: string | null;
} & (
  | {
      status: 'valid';
      slug: string;
      event_kind: 'thread' | 'system';
      definition: EventHookDefinition;
      content_hash: string;
    }
  | { status: 'invalid'; slug: string; definition: EventHookDefinition | null; problems: string[] }
  | { status: 'missing'; slug: string }
);

export async function fetchEventHooks(): Promise<EventHook[]> {
  const { hooks } = await json<{ hooks: EventHook[] }>(`${API}/hooks`);
  return hooks;
}

async function actOnHook(method: 'POST' | 'DELETE', path: string, body: object = {}): Promise<void> {
  const resp = await mutatingFetch(`${API}/hooks/${path}`, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  await throwIfNotOk(resp);
}

export function armEventHook(hook: EventHook): Promise<void> {
  const content_hash = hook.status === 'valid' ? hook.content_hash : null;
  return actOnHook('POST', `${encodeURIComponent(hook.slug)}/arm`, { content_hash });
}

export function disarmEventHook(hook: EventHook): Promise<void> {
  return actOnHook('POST', `${encodeURIComponent(hook.slug)}/disarm`);
}

export function removeEventHook(hook: EventHook): Promise<void> {
  return actOnHook('DELETE', encodeURIComponent(hook.slug));
}
