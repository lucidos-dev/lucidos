
import type { ChatGptPlanAccountInfo, ChatGptPlanStatus } from '../../api/client';

export type ChatGptPlanView =
  | {
    kind: 'signed-in';
    account: ChatGptPlanAccountInfo;
    pausedUntil: string | null;
    pauseRemainingMs: number | null;
  }
  | {
    kind: 'pending';
    expiresAt: string;
    signedIn: boolean;
  }
  | {
    kind: 'signed-out';
    notice: SignedOutNotice | null;
    registered: boolean;
  };

export type SignedOutNotice =
  | { kind: 'sign-in-again' }
  | { kind: 'failed'; error: string };

export function chatGptPlanView(status: ChatGptPlanStatus): ChatGptPlanView {
  const signIn = status.sign_in;
  if (signIn.state === 'pending') {
    return { kind: 'pending', expiresAt: signIn.expires_at, signedIn: status.account !== null };
  }
  if (status.account) {
    return {
      kind: 'signed-in',
      account: status.account,
      pausedUntil: status.paused_until,
      pauseRemainingMs: status.pause_remaining_ms,
    };
  }
  if (signIn.state === 'failed') {
    return {
      kind: 'signed-out',
      notice: { kind: 'failed', error: signIn.error },
      registered: status.registered,
    };
  }
  return {
    kind: 'signed-out',
    notice: status.sign_in_again ? { kind: 'sign-in-again' } : null,
    registered: status.registered,
  };
}
