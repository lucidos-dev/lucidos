import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** What `data/hooks/<slug>/hook.toml` declares. Mirrors the engine's
 *  `HookDefinition`. */
export interface EventHookDefinition {
  name: string;
  /** The hooked event's wire name. */
  event_type: string;
  condition?: unknown;
  /** The script's path inside the hook's folder. */
  script: string;
  timeout_secs?: number;
}

/** Where a hook stands with the owner. Mirrors the engine's `ArmState`.
 *  `changed` is armed, but its definition or script changed, broke or left. */
export type EventHookArmState = 'disarmed' | 'armed' | 'changed';

/** A hook's standing arming. Mirrors the engine's `ArmedHook`. */
export interface EventHookArming {
  slug: string;
  event_type: string;
  content_hash: string;
  armed_at: string;
}

/** One row of `GET /api/v1/hooks`: a folder under `data/hooks/`, or an armed
 *  hook whose folder is gone (`missing`), with its arming. */
export type EventHook = {
  arm_state: EventHookArmState;
  armed: EventHookArming | null;
  /** Why the owner cannot arm it now. `null` when they can. */
  arm_refusal: string | null;
  /** Why the owner cannot remove it here. `null` when they can. */
  remove_refusal: string | null;
} & (
  | {
      status: 'valid';
      slug: string;
      event_kind: 'thread' | 'system';
      definition: EventHookDefinition;
      /** The digest of the content now on disk, which an arming binds. */
      content_hash: string;
    }
  | { status: 'invalid'; slug: string; definition: EventHookDefinition | null; problems: string[] }
  | { status: 'missing'; slug: string }
);

export async function fetchEventHooks(): Promise<EventHook[]> {
  const { hooks } = await json<{ hooks: EventHook[] }>(`${API}/hooks`);
  return hooks;
}

/** Every verb takes a JSON body. Only an agent fills its `reason`. */
async function actOnHook(method: 'POST' | 'DELETE', path: string, body: object = {}): Promise<void> {
  const resp = await mutatingFetch(`${API}/hooks/${path}`, {
    method,
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  await throwIfNotOk(resp);
}

/** Arm the hook at the content the owner was shown. A hook that changed since
 *  then is refused, so the owner never arms content they did not see. */
export function armEventHook(hook: EventHook): Promise<void> {
  const content_hash = hook.status === 'valid' ? hook.content_hash : null;
  return actOnHook('POST', `${encodeURIComponent(hook.slug)}/arm`, { content_hash });
}

export function disarmEventHook(hook: EventHook): Promise<void> {
  return actOnHook('POST', `${encodeURIComponent(hook.slug)}/disarm`);
}

/** Delete the hook's folder and commit that, ending any arming. */
export function removeEventHook(hook: EventHook): Promise<void> {
  return actOnHook('DELETE', encodeURIComponent(hook.slug));
}
