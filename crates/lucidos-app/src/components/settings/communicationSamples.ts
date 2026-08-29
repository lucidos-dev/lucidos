import {
  showToast,
  showConfirm,
  showPrompt,
  dismissToast,
  progressDialog,
  TOAST_AUTO_DISMISS_MS,
  type ToastPlacement,
} from '../../store/store';
import { restartDialogState, appUpdateDialogState } from '../../store/progressDialogCopy';
import type { ProgressDialogState } from '../../store/types';
import type { AppUpdateRunning } from '../../utils/tauri';
import type { DropdownOption } from '../shared/Dropdown';
import type { WebhookIngressOutage } from '../../api/client';

/** Everything the communication-surface gallery fires, kept out of the page so
 *  the page is layout and this is content.
 *
 *  Every sample here is INERT. Nothing restarts, downloads, navigates, or
 *  writes a preference. That is the whole point of the gallery: the surfaces
 *  can be judged before a real flow is pointed at any of them. A source scan
 *  guards it (`__tests__/communication-samples-inert.test.ts`).
 *
 *  Copy is taken from the real call sites rather than invented, so a surface is
 *  judged at the lengths that actually occur. */

// --- Toast placement (temporary, see docs/temporary-measures.md) ---

/** Labels say what the shape DOES, not what it is called internally. The picker
 *  exists to answer "which of these looks right", and someone choosing between
 *  them has no other description to go on.
 *
 *  Typing `value` as the union is what keeps this list and the union in step: a
 *  typo here fails the build instead of offering an unpickable shape. */
export const TOAST_PLACEMENT_OPTIONS: (DropdownOption & { value: ToastPlacement })[] = [
  { value: 'bottom-right', label: 'Bottom-right corner', description: 'Desktop only, anchored to the window. Mobile keeps the top' },
  { value: 'top-bleed', label: 'Full width, top', description: 'One bar across both panes, under the header' },
  { value: 'bottom-bleed', label: 'Full width, bottom', description: 'One bar across both panes, over the composer' },
  { value: 'card', label: 'Centred card', description: 'One card centred on the window, crossing the divider' },
  { value: 'pane', label: 'Per pane', description: 'One column centred over each pane' },
];

// --- Toasts ---

/** Keyed for two reasons. A second press then REPLACES the sample rather than
 *  piling up duplicates, and a toast's own buttons can dismiss it by name. A
 *  sample with a dead button would misrepresent the shape it is there to show. */
const SAMPLE_SHORT = 'sample-toast-short';
const SAMPLE_LONG = 'sample-toast-long';
const SAMPLE_ACTIONS = 'sample-toast-actions';
const SAMPLE_ERROR = 'sample-toast-error';
const SAMPLE_PROGRESS = 'sample-toast-progress';

export function sampleShortToast(): void {
  showToast('Trigger saved.', 'success', { key: SAMPLE_SHORT, autoDismissMs: TOAST_AUTO_DISMISS_MS });
}

export function sampleLongToast(): void {
  showToast(
    'Merge conflict in "Put toasts where they originate" resolved automatically. '
    + 'The change applied on top of the current main and is ready to review.',
    'warning',
    { key: SAMPLE_LONG },
  );
}

export function sampleErrorToast(): void {
  showToast('Failed to apply change: the worktree has uncommitted edits.', 'error', { key: SAMPLE_ERROR });
}

export function sampleActionToast(): void {
  showToast('New engine version available.', 'info', {
    key: SAMPLE_ACTIONS,
    action: { label: 'Restart', onClick: () => dismissToast(SAMPLE_ACTIONS) },
    secondaryAction: { label: 'Later', onClick: () => dismissToast(SAMPLE_ACTIONS) },
  });
}

export function sampleProgressToast(): void {
  showToast('Downloading embedding model', 'info', {
    key: SAMPLE_PROGRESS,
    spinning: true,
    progress: 0.42,
    secondaryAction: { label: 'Cancel', onClick: () => dismissToast(SAMPLE_PROGRESS) },
  });
}

/** Four at once, staggered. A shape has to hold up as a STACK as well as
 *  one at a time, and toasts do arrive in bursts. */
export function sampleToastBurst(): void {
  sampleShortToast();
  setTimeout(sampleLongToast, 300);
  setTimeout(sampleErrorToast, 600);
  setTimeout(sampleActionToast, 900);
}

// --- Banners ---

/** The outage the ingress bar draws, at the shape of the real one.
 *
 *  One family down and the other healthy, because that is the case the bar
 *  exists for. A both-families sample would read as an obvious outage. The
 *  wording that matters is the wording for a path half of the internet still
 *  reaches. Eight hours is the span the real failure ran for.
 *
 *  The other two banners take their sample inline on the page: each is a couple
 *  of literal props. This one is a record, so it lives here with the rest of the
 *  content. */
export const SAMPLE_INGRESS_OUTAGE: WebhookIngressOutage = {
  webhook_name: 'github-ci',
  host: 'node.tailnet.ts.net',
  port: 8443,
  families: ['ipv4'],
  addresses: [
    {
      address: '203.0.113.7',
      family: 'ipv4',
      stage: 'ingress-unreachable',
      status: null,
      detail: 'could not connect: tls handshake eof',
    },
    { address: '2001:db8::1', family: 'ipv6', stage: 'healthy', status: 401, detail: null },
  ],
  down_since: '2026-08-26T22:10:00Z',
  down_secs: 28_800,
};

// --- Dialogs ---

export function sampleConfirmDanger(): void {
  void showConfirm('Delete the trigger "Daily digest"? This cannot be undone.', 'Delete', {
    title: 'Delete trigger',
  });
}

export function sampleConfirmDefault(): void {
  void showConfirm('Restart the engine to activate the applied changes?', 'Restart', {
    title: 'Restart engine',
    variant: 'default',
  });
}

export function samplePrompt(): void {
  void showPrompt('What should this thread be called?', {
    title: 'Rename thread',
    defaultValue: 'Put toasts where they originate',
  });
}

/** The three acknowledgements: something has already happened and the user has
 *  to see it. Nothing to decide, so each carries a lone [OK] and no Cancel. */
export function sampleAcknowledgeWedged(): void {
  void showConfirm(
    'New engine version pending, and rebuilding cannot deliver it: a build for '
    + 'this commit already succeeded without producing one.\n\n'
    + 'Relaunch the stack from your checkout.',
    'OK',
    { title: 'Cannot deliver the new version', acknowledge: true, variant: 'default' },
  );
}

export function sampleAcknowledgeDeferred(): void {
  void showConfirm(
    "Frontend change applied. It'll take effect when you switch to the new version.",
    'OK',
    { title: 'Frontend change applied', acknowledge: true, variant: 'default' },
  );
}

export function sampleAcknowledgeStranded(): void {
  void showConfirm(
    'Frontend change applied but not served yet: the build watch has not rebuilt.\n\n'
    + 'It will appear on its own once it does.',
    'OK',
    { title: 'Not served yet', acknowledge: true, variant: 'default' },
  );
}

/** The cross-thread consent prompt: another thread asks to put something on
 *  screen, and the user can decline. */
export function sampleConsentPrompt(): void {
  void showConfirm(
    '"Nightly digest" wants to open the Files panel.',
    'Open',
    { title: 'A thread wants to navigate', cancelLabel: 'Not now', variant: 'default' },
  );
}

// --- Progress dialog ---

/** Frames the packaged install really emits, in order. Real frames rather than
 *  invented copy: the dialog is then built by the shipped `appUpdateDialogState`,
 *  so the preview cannot drift from the run it depicts. The version is a fixed
 *  synthetic value, deliberately not the real release, so it can never collide
 *  with the `RELEASE` file being shipped. */
const INSTALL_FRAMES: AppUpdateRunning[] = [
  { version: '9.9.9', phase: 'checking' },
  { version: '9.9.9', phase: 'downloading', downloaded: 12_582_912, total: 68_157_440 },
  { version: '9.9.9', phase: 'downloading', downloaded: 43_646_976, total: 68_157_440 },
  { version: '9.9.9', phase: 'verifying' },
  { version: '9.9.9', phase: 'installing' },
  { version: '9.9.9', phase: 'restarting-services' },
  { version: '9.9.9', phase: 'relaunching' },
];

/** One install frame as the gallery shows it: the real dialog, with its Cancel
 *  swapped for a way out of the preview. The swap is what keeps the sample
 *  inert, since the real Cancel would abandon an actual run. */
function sampleInstallDialog(frame: AppUpdateRunning): ProgressDialogState {
  return {
    ...appUpdateDialogState(frame, stopSampleProgressDialog),
    cancel: { label: 'Close preview', onClick: stopSampleProgressDialog },
  };
}

let ticker: ReturnType<typeof setInterval> | null = null;

/** Stop the fake run and clear the slot. Idempotent, so the page can call it on
 *  unmount without knowing whether a run is going. */
export function stopSampleProgressDialog(): void {
  if (ticker !== null) {
    clearInterval(ticker);
    ticker = null;
  }
  progressDialog.value = { visible: false, title: '', message: '', progress: null };
}

/** Walk the install frames on a slow ticker, so every one can be looked at.
 *  The real run passes through some of these in under a second, which is
 *  exactly why the gallery does not reuse its timing. */
export function playSampleProgressDialog(): void {
  stopSampleProgressDialog();
  let i = 0;
  const render = () => { progressDialog.value = sampleInstallDialog(INSTALL_FRAMES[i]); };
  render();
  ticker = setInterval(() => {
    i += 1;
    if (i >= INSTALL_FRAMES.length) {
      stopSampleProgressDialog();
      return;
    }
    render();
  }, 2000);
}

/** Hold one phase still, for looking at the layout rather than the sequence. */
export function showSampleProgressPhase(index: number): void {
  stopSampleProgressDialog();
  const frame = INSTALL_FRAMES[Math.min(Math.max(index, 0), INSTALL_FRAMES.length - 1)];
  progressDialog.value = sampleInstallDialog(frame);
}

/** The engine restart, which shares the dialog and has no honest percentage.
 *  Built from the shipped builder, so this is the real thing with a way out. */
export function sampleRestartDialog(): void {
  stopSampleProgressDialog();
  progressDialog.value = {
    ...restartDialogState(true),
    cancel: { label: 'Close preview', onClick: stopSampleProgressDialog },
  };
}
