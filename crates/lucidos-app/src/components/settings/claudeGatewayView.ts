
import type { ClaudeGatewayAccountInfo, ClaudeGatewayStatus } from '../../api/client';

export type ClaudeGatewayView =
  | {
    kind: 'signed-in';
    account: ClaudeGatewayAccountInfo;
    telemetry: boolean;
  }
  | {
    kind: 'pending';
    userCode: string;
    signInPage: string;
    expiresAt: string;
    signedIn: boolean;
  }
  | {
    kind: 'signed-out';
    notice: SignedOutNotice | null;
  };

export type SignedOutNotice =
  | { kind: 'sign-in-again' }
  | { kind: 'failed'; error: string };

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

export function initialGatewayUrl(stored: string, suggested: string | null): string {
  return stored || suggested || '';
}
