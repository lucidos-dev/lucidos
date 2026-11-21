import { API, json, mutatingFetch, throwIfNotOk } from './_core';

/** Where the current Sign in with ChatGPT attempt stands. Mirrors the engine's
 *  `chatgpt_plan_sign_in::SignInState`. */
export type ChatGptPlanSignInState =
  | { state: 'idle' }
  /** Waiting for the browser to come back from ChatGPT until `expires_at`. */
  | { state: 'pending'; expires_at: string }
  | { state: 'failed'; error: string };

/** The stored sign-in: who and when, never a token. */
export interface ChatGptPlanAccountInfo {
  email: string | null;
  signed_in_at: string;
}

/** Mirrors the engine's `ChatGptPlanStatus`. */
export interface ChatGptPlanStatus {
  account: ChatGptPlanAccountInfo | null;
  /** OpenAI refused the last sign-in for good, and none has been made since. */
  sign_in_again: boolean;
  /** A registration from an earlier sign-in is kept, so only that account may
   *  sign in until the user forgets it. */
  registered: boolean;
  sign_in: ChatGptPlanSignInState;
  /** ISO timestamp when plan requests resume after the usage limit. */
  paused_until: string | null;
  /** How long the pause has left, by the engine's clock. */
  pause_remaining_ms: number | null;
}

/** A started sign-in: the ChatGPT page the browser must open. */
export interface ChatGptPlanStartedSignIn {
  authorize_url: string;
  expires_at: string;
}

const CHATGPT_PLAN = `${API}/providers/chatgpt-plan`;

export function getChatGptPlanStatus(): Promise<ChatGptPlanStatus> {
  return json<ChatGptPlanStatus>(CHATGPT_PLAN);
}

/** Start a sign-in. A refusal, such as a browser the callback cannot reach,
 *  throws with the engine's reason. */
export async function startChatGptPlanSignIn(): Promise<ChatGptPlanStartedSignIn> {
  const resp = await mutatingFetch(`${CHATGPT_PLAN}/sign-in`, { method: 'POST' });
  await throwIfNotOk(resp);
  return resp.json() as Promise<ChatGptPlanStartedSignIn>;
}

/** Stop any sign-in in progress and forget the stored one. `forgetAccount`
 *  forgets the registration too, so the next sign-in may use another account. */
export async function signOutOfChatGptPlan(forgetAccount = false): Promise<void> {
  const query = forgetAccount ? '?forget_account=true' : '';
  const resp = await mutatingFetch(`${CHATGPT_PLAN}/sign-out${query}`, { method: 'POST' });
  await throwIfNotOk(resp);
}
