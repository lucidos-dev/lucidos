import { signal } from '@preact/signals';
import {
  getClaudeGatewayStatus,
  signOutOfClaudeGateway,
  startClaudeGatewaySignIn,
  type ClaudeGatewayStatus,
} from '../../api/client';
import { showToast } from '../store';
import { failedIfFresh, setLoadingIfFresh, type Loadable } from '../types';
import { errorDetail } from '../../utils/errorDetail';

export const claudeGatewayStatus = signal<Loadable<ClaudeGatewayStatus>>({ status: 'not-loaded' });

let loadSeq = 0;

export async function loadClaudeGatewayStatus(): Promise<void> {
  const seq = ++loadSeq;
  setLoadingIfFresh(claudeGatewayStatus);
  try {
    const status = await getClaudeGatewayStatus();
    if (seq !== loadSeq) return;
    claudeGatewayStatus.value = { status: 'loaded', data: status };
  } catch (e) {
    if (seq !== loadSeq) return;
    claudeGatewayStatus.value = failedIfFresh(claudeGatewayStatus.value, e);
  }
}

export function refreshClaudeGatewayStatusIfLoaded(): void {
  if (claudeGatewayStatus.value.status !== 'not-loaded') void loadClaudeGatewayStatus();
}

export async function submitClaudeGatewaySignIn(url: string): Promise<void> {
  try {
    await startClaudeGatewaySignIn(url);
  } catch (e) {
    showToast(`Could not start the sign-in to the Claude gateway ${url}: ${errorDetail(e)}`, 'error');
  }
  await loadClaudeGatewayStatus();
}

export async function submitClaudeGatewaySignOut(): Promise<void> {
  try {
    await signOutOfClaudeGateway();
  } catch (e) {
    showToast(`Could not sign out of the Claude gateway: ${errorDetail(e)}`, 'error');
  }
  await loadClaudeGatewayStatus();
}
