import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** A sender's signature scheme, as data. Mirrors the engine's `HmacConfig`.
 *
 *  `credential` NAMES a saved credential. The secret itself never travels
 *  through here, in either direction. */
export interface WebhookHmac {
  credential: string;
  signature_header: string;
  algorithm?: 'sha256' | 'sha1';
  encoding?: 'hex' | 'base64';
  /** Literal prefix to strip off the header value, e.g. `sha256=`. */
  prefix?: string;
  /** Key to read out of a comma-separated `k=v` header, e.g. Stripe's `v1`. */
  signature_key?: string;
  timestamp_header?: string;
  timestamp_key?: string;
  /** The signed string. `{body}` and `{timestamp}` are substituted. */
  template?: string;
  tolerance_secs?: number;
}

/** How a hook recognises a delivery it already emitted. Mirrors the engine's
 *  `DedupeConfig`.
 *
 *  Absent on the webhook means it does not dedupe, which is the default: every
 *  arrival emits, so the log keeps a sender's retries. */
export interface WebhookDedupe {
  /** Header carrying the sender's delivery id, e.g. `X-GitHub-Delivery`.
   *  Absent, and the key is a digest of the body. */
  header?: string;
  /** How long a claim holds. `0` switches deduping off. */
  window_secs: number;
}

/** One configured webhook. Carries no secret: the token is returned once at
 *  create and only its digest is stored. */
export interface Webhook {
  id: string;
  name: string;
  /** The domain event every delivery emits. Pinned, so a caller cannot pick. */
  event_type: string;
  enabled: boolean;
  signed: boolean;
  hmac: WebhookHmac | null;
  dedupe: WebhookDedupe | null;
  /** Request headers copied into the event payload, under `headers`.
   *  An allow-list: the engine refuses `Authorization` and the hook's own
   *  signature header, since the event log is append-only. */
  headers: string[];
  created_at: string;
  /** Path a sender posts to, under whatever host the hook socket is exposed
   *  on. The engine knows no public hostname, so it states the path alone. */
  delivery_path: string;
}

/** A create response: the webhook, plus its token in readable form for the
 *  only time it ever is.
 *
 *  `token` is absent for a SIGNED webhook, which authenticates by signature
 *  alone. A sender like GitHub cannot present a bearer token, so pinning one
 *  would make the hook refuse every real delivery. */
export interface CreatedWebhook extends Webhook {
  token?: string;
}

export function fetchWebhooks(): Promise<Webhook[]> {
  return json(`${API}/webhooks`);
}

export async function createWebhook(input: {
  name: string;
  event_type: string;
  hmac?: WebhookHmac;
}): Promise<CreatedWebhook> {
  const resp = await mutatingFetch(`${API}/webhooks`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(input),
  });
  await throwIfNotOk(resp);
  return resp.json();
}

/** Change a webhook. Every field is optional and an omitted one is untouched. */
export async function updateWebhook(
  id: string,
  changes: { name?: string; event_type?: string; enabled?: boolean },
): Promise<Webhook> {
  const resp = await mutatingFetch(`${API}/webhooks/${encodeURIComponent(id)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(changes),
  });
  await throwIfNotOk(resp);
  return resp.json();
}

export async function deleteWebhook(id: string): Promise<void> {
  const resp = await mutatingFetch(`${API}/webhooks/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
  await throwIfNotOk(resp);
}
