import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export type ChatGptPlanSignInState =
  | { state: 'idle' }
  | { state: 'pending'; expires_at: string }
  | { state: 'failed'; error: string };

export interface ChatGptPlanAccountInfo {
  email: string | null;
  signed_in_at: string;
}

export interface ChatGptPlanStatus {
  account: ChatGptPlanAccountInfo | null;
  sign_in_again: boolean;
  registered: boolean;
  sign_in: ChatGptPlanSignInState;
  paused_until: string | null;
  pause_remaining_ms: number | null;
}

export interface ChatGptPlanStartedSignIn {
  authorize_url: string;
  expires_at: string;
}

const CHATGPT_PLAN = `${API}/providers/chatgpt-plan`;

export function getChatGptPlanStatus(): Promise<ChatGptPlanStatus> {
  return json<ChatGptPlanStatus>(CHATGPT_PLAN);
}

export async function startChatGptPlanSignIn(): Promise<ChatGptPlanStartedSignIn> {
  const resp = await mutatingFetch(`${CHATGPT_PLAN}/sign-in`, { method: 'POST' });
  await throwIfNotOk(resp);
  return resp.json() as Promise<ChatGptPlanStartedSignIn>;
}

export async function signOutOfChatGptPlan(forgetAccount = false): Promise<void> {
  const query = forgetAccount ? '?forget_account=true' : '';
  const resp = await mutatingFetch(`${CHATGPT_PLAN}/sign-out${query}`, { method: 'POST' });
  await throwIfNotOk(resp);
}
