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
  /** When a delivery last verified and emitted. `null` means never.
   *
   *  This field and the two below make silence readable. "Arrived and was
   *  refused" and "never arrived" produce the same symptom, no events, and
   *  have completely different causes. */
  last_accepted_at: string | null;
  /** When a delivery last arrived and was turned away. `null` means never. */
  last_refused_at: string | null;
  /** Why that refusal happened. Shown to the workspace owner only: the sender
   *  gets a bare 401, since naming the reason helps whoever is guessing. */
  last_refusal_reason: string | null;
  /** Path a sender posts to, under whatever host the hook socket is exposed
   *  on. The engine knows no public hostname, so it states the path alone. */
  delivery_path: string;
}

/** Where the credential's value comes from, when the request brings one.
 *
 *  Which side invents a shared secret depends on the sender. GitHub takes
 *  whatever the receiver puts in its webhook form, so `generate` is right
 *  there. Slack and Stripe issue their own, so those can only be `provided`.
 *
 *  Both write one credential and then name it on the hook. Omit the field
 *  entirely and `hmac.credential` must already name a saved one. */
export type WebhookSigningSecret =
  | { mode: 'generate' }
  | { mode: 'provided'; value: string };

/** A webhook, plus whatever secret this one response is the only sight of.
 *
 *  `token` is absent for a SIGNED webhook, which authenticates by signature
 *  alone. A sender like GitHub cannot present a bearer token, so pinning one
 *  would make the hook refuse every real delivery.
 *
 *  `signing_secret` is present only when the request asked to generate one. It
 *  is the value to paste into the sender's own webhook form. */
export interface WebhookWithToken extends Webhook {
  token?: string;
  signing_secret?: string;
}

/** Which address family an ingress probe could not reach. */
export type WebhookIngressFamily = 'ipv4' | 'ipv6';

/** One standing outage of the public path every webhook shares.
 *
 *  It names no webhook. The probe picks one hook as its target, and what failed
 *  is the ingress in front of all of them. */
export interface WebhookIngressOutage {
  host: string;
  port: number;
  /** The families that could not be reached: `ipv4`, `ipv6`, or both. */
  families: WebhookIngressFamily[];
  /** RFC 3339, from the engine's own declaration of the outage. */
  down_since: string;
  /** How long it has been down, measured by the database. */
  down_secs: number;
}

/** The health of the public delivery path, for a cold page load.
 *
 *  SSE carries the two `WebhookIngress*` events while the app is open. This is
 *  what a client that just started reads instead of replaying the timeline. */
export interface WebhookIngress {
  /** `null` when the path is healthy, was never probed, or no hook is enabled. */
  degraded: WebhookIngressOutage | null;
}

export function fetchWebhooks(): Promise<Webhook[]> {
  return json(`${API}/webhooks`);
}

export function fetchWebhookIngress(): Promise<WebhookIngress> {
  return json(`${API}/webhooks/ingress`);
}

export async function createWebhook(input: {
  name: string;
  event_type: string;
  hmac?: WebhookHmac;
  signing_secret?: WebhookSigningSecret;
}): Promise<WebhookWithToken> {
  const resp = await mutatingFetch(`${API}/webhooks`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(input),
  });
  await throwIfNotOk(resp);
  return resp.json();
}

/** Change a webhook. Every field is optional and an omitted one is untouched.
 *
 *  `hmac` is the exception, and its three states are deliberate: omit it to
 *  keep the stored config, pass an object to sign from now on, and pass `null`
 *  to stop signing. Clearing it mints a bearer token, returned once, since a
 *  hook always carries exactly one verifier.
 *
 *  `signing_secret` rotates the value the hook verifies with. It works on its
 *  own, which is the cheapest gesture: change the secret and touch nothing
 *  else. */
export async function updateWebhook(
  id: string,
  changes: {
    name?: string;
    event_type?: string;
    enabled?: boolean;
    hmac?: WebhookHmac | null;
    signing_secret?: WebhookSigningSecret;
  },
): Promise<WebhookWithToken> {
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
