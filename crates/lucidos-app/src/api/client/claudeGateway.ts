import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** Where the current Claude gateway sign-in stands. Mirrors the engine's
 *  `SignInState`. Carries the code the user enters, never a token. */
export type ClaudeGatewaySignInState =
  | { state: 'idle' }
  | {
    state: 'pending';
    issuer: string;
    user_code: string;
    verification_uri: string;
    verification_uri_complete: string | null;
    /** ISO timestamp after which the code no longer works. */
    expires_at: string;
  }
  | { state: 'failed'; issuer: string; error: string };

/** The stored sign-in: who and where, never a token. */
export interface ClaudeGatewayAccountInfo {
  issuer: string;
  email: string | null;
  signed_in_at: string;
}

/** Mirrors the engine's `ClaudeGatewayStatus`. */
export interface ClaudeGatewayStatus {
  /** The `claude_gateway_url` preference. */
  url: string | null;
  /** The gateway Claude Code's managed settings name on this machine. An
   *  offer only: nothing signs in to it until the user confirms. */
  suggested_url: string | null;
  account: ClaudeGatewayAccountInfo | null;
  /** The gateway rejected the last sign-in, and none has been made since. */
  sign_in_again: boolean;
  /** The signed-in gateway's managed settings turn usage telemetry on, and
   *  the engine's environment does not opt out. */
  telemetry: boolean;
  sign_in: ClaudeGatewaySignInState;
}

const CLAUDE_GATEWAY = `${API}/providers/claude-gateway`;

export function getClaudeGatewayStatus(): Promise<ClaudeGatewayStatus> {
  return json<ClaudeGatewayStatus>(CLAUDE_GATEWAY);
}

/** Start a device sign-in. A `url` is stored as the gateway URL first, which
 *  signs out of a different gateway. Answers with the code to enter. */
export async function startClaudeGatewaySignIn(url?: string): Promise<ClaudeGatewaySignInState> {
  const resp = await mutatingFetch(`${CLAUDE_GATEWAY}/sign-in`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(url === undefined ? {} : { url }),
  });
  await throwIfNotOk(resp);
  return resp.json() as Promise<ClaudeGatewaySignInState>;
}

/** Stop any sign-in in progress and forget the stored one. */
export async function signOutOfClaudeGateway(): Promise<void> {
  const resp = await mutatingFetch(`${CLAUDE_GATEWAY}/sign-out`, { method: 'POST' });
  await throwIfNotOk(resp);
}
