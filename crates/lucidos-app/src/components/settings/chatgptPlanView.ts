/**
 * What the ChatGPT plan card shows, as a pure function of the engine's status,
 * so every state is testable without the Providers page.
 */

import type { ChatGptPlanAccountInfo, ChatGptPlanStatus } from '../../api/client';

export type ChatGptPlanView =
  | {
    kind: 'signed-in';
    account: ChatGptPlanAccountInfo;
    /** When plan requests resume, while the usage limit pauses them. */
    pausedUntil: string | null;
    /** How long the pause has left, by the engine's clock. */
    pauseRemainingMs: number | null;
  }
  | {
    kind: 'pending';
    expiresAt: string;
    /** A sign-in is stored too, so stopping this one must not sign out. */
    signedIn: boolean;
  }
  | {
    kind: 'signed-out';
    /** Why the user must sign in, when it is not a first sign-in. */
    notice: SignedOutNotice | null;
    /** Only the registered account may sign in until the user forgets it. */
    registered: boolean;
  };

export type SignedOutNotice =
  | { kind: 'sign-in-again' }
  | { kind: 'failed'; error: string };

/** An attempt in progress wins over a stored account, since starting one is
 *  what the user did last. The failure of an attempt outranks the older "sign
 *  in again", which it followed. */
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
