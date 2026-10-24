/**
 * The Claude gateway sign-in as Settings shows it: the stored account, the
 * sign-in in progress, and the gateway Claude Code's managed settings suggest.
 *
 * The engine owns the sign-in. This module reads its status and sends intent.
 * Every change reaches it as an SSE event (`ClaudeGatewaySignedIn`,
 * `ClaudeGatewaySignedOut`, `ClaudeGatewaySignInStarted`,
 * `ClaudeGatewaySignInCanceled`, `ClaudeGatewaySignInFailed`,
 * `ClaudeGatewayManagedSettingsChanged`), which reloads it.
 */
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

/** Reload only if Settings already asked for it. */
export function refreshClaudeGatewayStatusIfLoaded(): void {
  if (claudeGatewayStatus.value.status !== 'not-loaded') void loadClaudeGatewayStatus();
}

/** Start a sign-in to `url`, storing it as the gateway URL. The status then
 *  holds the code to enter. */
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
