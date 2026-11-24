import { signal } from '@preact/signals';
import {
  getChatGptPlanStatus,
  signOutOfChatGptPlan,
  startChatGptPlanSignIn,
  type ChatGptPlanStatus,
} from '../../api/client';
import { showConfirm, showToast } from '../store';
import { failedIfFresh, setLoadingIfFresh, type Loadable } from '../types';
import { errorDetail } from '../../utils/errorDetail';
import { openUrlOutsideApp } from './artifacts';
import { chatGptPlanWelcomeShown, markChatGptPlanWelcomeShown } from './preferences';

export const CHATGPT_USAGE_URL = 'https://chatgpt.com/settings/usage';

export const chatGptPlanStatus = signal<Loadable<ChatGptPlanStatus>>({ status: 'not-loaded' });

let loadSeq = 0;
let startedHere = false;

export async function loadChatGptPlanStatus(): Promise<void> {
  const seq = ++loadSeq;
  setLoadingIfFresh(chatGptPlanStatus);
  try {
    const status = await getChatGptPlanStatus();
    if (seq !== loadSeq) return;
    chatGptPlanStatus.value = { status: 'loaded', data: status };
    welcomeIfFirstSignIn(status);
  } catch (e) {
    if (seq !== loadSeq) return;
    chatGptPlanStatus.value = failedIfFresh(chatGptPlanStatus.value, e);
  }
}

export function refreshChatGptPlanStatusIfLoaded(): void {
  if (chatGptPlanStatus.value.status !== 'not-loaded') void loadChatGptPlanStatus();
}

export async function submitChatGptPlanSignIn(): Promise<void> {
  try {
    const started = await startChatGptPlanSignIn();
    startedHere = true;
    openUrlOutsideApp(started.authorize_url);
  } catch (e) {
    showToast(`Could not start Sign in with ChatGPT: ${errorDetail(e)}`, 'error');
  }
  await loadChatGptPlanStatus();
}

export async function submitChatGptPlanSignOut(): Promise<void> {
  try {
    await signOutOfChatGptPlan();
  } catch (e) {
    showToast(`Could not sign out of ChatGPT: ${errorDetail(e)}`, 'error');
  }
  await loadChatGptPlanStatus();
}

export async function submitChatGptPlanForgetAccount(): Promise<void> {
  const confirmed = await showConfirm(
    'Lucidos forgets the ChatGPT account it signed in with, and the next sign-in '
      + 'registers it with OpenAI again. Any limit you set for this app in your '
      + 'ChatGPT settings stays with the old registration.',
    'Use another account',
    { title: 'Use another ChatGPT account?', variant: 'danger' },
  );
  if (!confirmed) return;
  try {
    await signOutOfChatGptPlan(true);
  } catch (e) {
    showToast(`Could not forget the ChatGPT account: ${errorDetail(e)}`, 'error');
  }
  await loadChatGptPlanStatus();
}

function welcomeIfFirstSignIn(status: ChatGptPlanStatus): void {
  if (!startedHere || status.sign_in.state === 'pending') return;
  startedHere = false;
  if (!status.account || chatGptPlanWelcomeShown()) return;
  void markChatGptPlanWelcomeShown();
  void showConfirm(
    'Eligible usage in this app uses your ChatGPT plan. Manage usage in your ChatGPT settings.',
    'Got it',
    {
      title: "You're using your ChatGPT plan",
      variant: 'default',
      acknowledge: true,
      extraAction: { label: 'Manage usage', onClick: () => openUrlOutsideApp(CHATGPT_USAGE_URL) },
    },
  );
}
