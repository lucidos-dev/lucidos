import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export type ClaudeGatewaySignInState =
  | { state: 'idle' }
  | {
    state: 'pending';
    issuer: string;
    user_code: string;
    verification_uri: string;
    verification_uri_complete: string | null;
    expires_at: string;
  }
  | { state: 'failed'; issuer: string; error: string };

export interface ClaudeGatewayAccountInfo {
  issuer: string;
  email: string | null;
  signed_in_at: string;
}

export interface ClaudeGatewayStatus {
  url: string | null;
  suggested_url: string | null;
  account: ClaudeGatewayAccountInfo | null;
  sign_in_again: boolean;
  telemetry: boolean;
  sign_in: ClaudeGatewaySignInState;
}

const CLAUDE_GATEWAY = `${API}/providers/claude-gateway`;

export function getClaudeGatewayStatus(): Promise<ClaudeGatewayStatus> {
  return json<ClaudeGatewayStatus>(CLAUDE_GATEWAY);
}

export async function startClaudeGatewaySignIn(url?: string): Promise<ClaudeGatewaySignInState> {
  const resp = await mutatingFetch(`${CLAUDE_GATEWAY}/sign-in`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(url === undefined ? {} : { url }),
  });
  await throwIfNotOk(resp);
  return resp.json() as Promise<ClaudeGatewaySignInState>;
}

export async function signOutOfClaudeGateway(): Promise<void> {
  const resp = await mutatingFetch(`${CLAUDE_GATEWAY}/sign-out`, { method: 'POST' });
  await throwIfNotOk(resp);
}
