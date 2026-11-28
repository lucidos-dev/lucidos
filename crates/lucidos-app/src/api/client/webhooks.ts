import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export interface WebhookHmac {
  credential: string;
  signature_header: string;
  algorithm?: 'sha256' | 'sha1';
  encoding?: 'hex' | 'base64';
  prefix?: string;
  signature_key?: string;
  timestamp_header?: string;
  timestamp_key?: string;
  template?: string;
  tolerance_secs?: number;
}

export interface WebhookDedupe {
  header?: string;
  window_secs: number;
}

export interface Webhook {
  id: string;
  name: string;
  event_type: string;
  enabled: boolean;
  signed: boolean;
  hmac: WebhookHmac | null;
  dedupe: WebhookDedupe | null;
  headers: string[];
  created_at: string;
  last_accepted_at: string | null;
  last_refused_at: string | null;
  last_refusal_reason: string | null;
  refusal_run?: WebhookRefusalRun;
  delivery_path: string;
}

export type WebhookSigningSecret =
  | { mode: 'generate' }
  | { mode: 'provided'; value: string };

export interface WebhookWithToken extends Webhook {
  token?: string;
  signing_secret?: string;
}

export interface WebhookRefusalRun {
  refusals: number;
  since: string | null;
  reasons: Record<string, number>;
}

export type WebhookRefusalCause = 'disabled' | 'verification';

export interface WebhookRefusal {
  webhook_id: string;
  webhook_name: string;
  enabled: boolean;
  cause: WebhookRefusalCause;
  refusals: number;
  reasons: Record<string, number>;
  refusing_since: string;
  refusing_secs: number;
}

export interface WebhookRefusals {
  refusing: WebhookRefusal[];
}

export type WebhookIngressFamily = 'ipv4' | 'ipv6';

export type WebhookIngressStage =
  | 'healthy'
  | 'ingress-unreachable'
  | 'backend-unreachable'
  | 'route-missing'
  | 'unexpected-responder'
  | 'local-stack-unavailable'
  | 'local-egress-blocked';

export interface WebhookIngressAddress {
  address: string;
  family: WebhookIngressFamily;
  stage: WebhookIngressStage;
  status: number | null;
  detail: string | null;
}

export interface WebhookIngressOutage {
  webhook_name: string;
  host: string;
  port: number;
  families: WebhookIngressFamily[];
  addresses: WebhookIngressAddress[];
  down_since: string;
  down_secs: number;
}

export interface WebhookIngress {
  degraded: WebhookIngressOutage | null;
  unverified: boolean;
}

export function fetchWebhooks(): Promise<Webhook[]> {
  return json(`${API}/webhooks`);
}

export function fetchWebhookIngress(): Promise<WebhookIngress> {
  return json(`${API}/webhooks/ingress`);
}

export function fetchWebhookRefusals(): Promise<WebhookRefusals> {
  return json(`${API}/webhooks/refusals`);
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
