import { effect } from '@preact/signals';
import { focusedThreadId, setFocusedThread, showToast, threadsLoaded } from '../store';
import type { WithheldThread } from '../restoreGuard';
import { whenWindowSeen } from '../windowPeers';
import { windowId } from '../../utils/windowStorage';
import { formatThreadLabel } from './thread-label';
import { homeThreadId } from './homeThread';
import { focusThread, focusThreadOrBootstrap } from './threads';

export function coldStartFocus(opts: {
  storedFocus: boolean;
  focusedNow: string | null;
  homeId: string | null;
}): string | null {
  if (opts.storedFocus || opts.focusedNow) return null;
  return opts.homeId;
}

export function openHomeOnColdStart(storedFocus: boolean): () => void {
  let decided = false;
  const stop = effect(() => {
    if (decided || !threadsLoaded.value) return;
    decided = true;
    const target = coldStartFocus({
      storedFocus,
      focusedNow: focusedThreadId.peek(),
      homeId: homeThreadId.peek(),
    });
    if (target) focusThread(target, { revealPane: false });
  });
  return stop;
}

export function releaseWithheldToLiveWindow(withheld: WithheldThread | null): () => void {
  if (withheld === null || withheld.armedBy === windowId()) return () => {};
  return whenWindowSeen(withheld.armedBy, () => {
    if (focusedThreadId.peek() === null) setFocusedThread(withheld.threadId);
  });
}

export function offerWithheldThread(threadId: string | null): () => void {
  if (threadId === null) return () => {};
  let offered = false;
  return effect(() => {
    if (offered || !threadsLoaded.value) return;
    offered = true;
    if (focusedThreadId.peek() === threadId) return;
    showToast(
      `Lucidos closed while ${formatThreadLabel(threadId)} was open, so it started on Home.`,
      'info',
      {
        title: 'Started on Home',
        action: { label: 'Open thread', onClick: () => focusThreadOrBootstrap(threadId), deliberate: true },
      },
    );
  });
}
