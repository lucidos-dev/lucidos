import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export interface EventHookDefinition {
  name: string;
  event_type: string;
  condition?: unknown;
  script: string;
  timeout_secs?: number;
  arguments?: Record<string, { label: string; choices_script: string }>;
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
  plugin_id?: string;
} & (
  | {
      status: 'valid';
      slug: string;
      event_kind: 'thread' | 'system';
      definition: EventHookDefinition;
      content_hash: string;
      choices_timeout_secs?: number;
    }
  | { status: 'invalid'; slug: string; definition: EventHookDefinition | null; problems: string[] }
  | { status: 'missing'; slug: string }
);

export async function fetchEventHooks(): Promise<EventHook[]> {
  const { hooks } = await json<{ hooks: EventHook[] }>(`${API}/hooks`);
  return hooks;
}

export interface HookChoice {
  value: string;
  label: string;
}

export interface HookInputOffer {
  hook: string;
  argument: string;
  label: string;
  creation?: { label: string; key: string; max_bytes: number };
  choices_timeout_secs: number;
  choices: HookChoice[];
}

export function isHookInputOffer(input: unknown): input is HookInputOffer {
  if (!input || typeof input !== 'object') return false;
  const candidate = input as Partial<HookInputOffer>;
  return typeof candidate.hook === 'string'
    && typeof candidate.argument === 'string'
    && typeof candidate.label === 'string'
    && Number.isInteger(candidate.choices_timeout_secs)
    && (candidate.creation === undefined || (candidate.creation !== null
      && typeof candidate.creation.label === 'string'
      && typeof candidate.creation.key === 'string'
      && Number.isInteger(candidate.creation.max_bytes)))
    && Array.isArray(candidate.choices)
    && candidate.choices.every(choice => choice && typeof choice.value === 'string' && typeof choice.label === 'string');
}

export async function cancelHookInput(eventId: string): Promise<void> {
  await actOnHook('POST', `pending/${encodeURIComponent(eventId)}/cancel`);
}

const CHOICE_REQUEST_GRACE_MS = 5_000;

export async function fetchHookChoices(
  slug: string,
  argument: string,
  query: string,
  scriptTimeoutSecs: number,
  signal: AbortSignal,
): Promise<HookChoice[]> {
  const path = `${API}/hooks/${encodeURIComponent(slug)}/arguments/${encodeURIComponent(argument)}/choices`;
  const params = new URLSearchParams({ query });
  const deadline = scriptTimeoutSecs * 1000 + CHOICE_REQUEST_GRACE_MS;
  const { choices } = await json<{ choices: HookChoice[] }>(`${path}?${params}`, { signal }, deadline);
  return choices;
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
