/**
 * What the Claude gateway card shows, as a pure function of the engine's
 * status, so every state is testable without the Providers page.
 */

import type { ClaudeGatewayAccountInfo, ClaudeGatewayStatus } from '../../api/client';

export type ClaudeGatewayView =
  | {
    kind: 'signed-in';
    account: ClaudeGatewayAccountInfo;
    /** The gateway collects usage telemetry, which the card says. */
    telemetry: boolean;
  }
  | {
    kind: 'pending';
    userCode: string;
    /** The page to open: the one carrying the code when the gateway gave it. */
    signInPage: string;
    expiresAt: string;
    /** A sign-in is stored too, so stopping this one must not sign out. */
    signedIn: boolean;
  }
  | {
    kind: 'signed-out';
    /** Why the user must sign in, when it is not a first sign-in. */
    notice: SignedOutNotice | null;
  };

export type SignedOutNotice =
  | { kind: 'sign-in-again' }
  | { kind: 'failed'; error: string };

/** A sign-in in progress wins over a stored account, since starting one is
 *  what the user did last. The failure of a sign-in outranks the older
 *  "sign in again", which it followed. */
export function claudeGatewayView(status: ClaudeGatewayStatus): ClaudeGatewayView {
  const signIn = status.sign_in;
  if (signIn.state === 'pending') {
    return {
      kind: 'pending',
      userCode: signIn.user_code,
      signInPage: signIn.verification_uri_complete ?? signIn.verification_uri,
      expiresAt: signIn.expires_at,
      signedIn: status.account !== null,
    };
  }
  if (status.account) {
    return { kind: 'signed-in', account: status.account, telemetry: status.telemetry };
  }
  if (signIn.state === 'failed') {
    return { kind: 'signed-out', notice: { kind: 'failed', error: signIn.error } };
  }
  return {
    kind: 'signed-out',
    notice: status.sign_in_again ? { kind: 'sign-in-again' } : null,
  };
}

/** The URL the field starts on: the stored one, else the managed suggestion. */
export function initialGatewayUrl(stored: string, suggested: string | null): string {
  return stored || suggested || '';
}
