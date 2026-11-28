import {
  showToast,
  showConfirm,
  showPrompt,
  dismissToast,
  progressDialog,
  TOAST_AUTO_DISMISS_MS,
} from '../../store/store';
import { restartDialogState, appUpdateDialogState } from '../../store/progressDialogCopy';
import type { ProgressDialogState } from '../../store/types';
import type { AppUpdateRunning } from '../../utils/tauri';
import type { WebhookIngressOutage, WebhookRefusal } from '../../api/client';
import type { EventCondition } from '../../store/store';
import type { SlownessEpisode } from '../../store/actions/slowness';



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

export function sampleToastBurst(): void {
  sampleShortToast();
  setTimeout(sampleLongToast, 300);
  setTimeout(sampleErrorToast, 600);
  setTimeout(sampleActionToast, 900);
}


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

export const SAMPLE_WEBHOOK_REFUSAL: WebhookRefusal = {
  webhook_id: '6f1c0f3e-0000-4000-8000-000000000001',
  webhook_name: 'GitHub workflow runs',
  enabled: false,
  cause: 'disabled',
  refusals: 42,
  reasons: { disabled: 42 },
  refusing_since: '2026-09-02T04:41:06Z',
  refusing_secs: 1_555_200,
};

export const SAMPLE_MEMORY_EPISODE: SlownessEpisode = {
  state: 'slow',
  episode_id: 'sample',
  reason: 'memory',
  top_users: [
    { name: 'Google Chrome', bytes: 7_000_000_000, kind: 'app' },
    { name: 'Lucidos', bytes: 1_400_000_000, kind: 'lucidos' },
    { name: 'Slack', bytes: 600_000_000, kind: 'app' },
  ],
};

export const SAMPLE_UNCLEAR_EPISODE: SlownessEpisode = {
  state: 'slow',
  episode_id: 'sample-unclear',
  reason: 'unclear',
  busiest_apps: [
    { name: 'Xcode', percent: 40, kind: 'app' },
    { name: 'Lucidos', percent: 12, kind: 'lucidos' },
  ],
  slow_workspaces: ['dev'],
};

export const SAMPLE_DATABASE_EPISODE: SlownessEpisode = {
  state: 'slow',
  episode_id: 'sample-database',
  reason: 'database',
  problem: 'not_answering',
  slow_workspaces: ['dev'],
};

export const SAMPLE_DISK_EPISODE: SlownessEpisode = {
  state: 'slow',
  episode_id: 'sample-disk',
  reason: 'disk',
  free_bytes: 4e8,
};

export const SAMPLE_WEBHOOK_VERIFICATION_REFUSAL: WebhookRefusal = {
  webhook_id: '6f1c0f3e-0000-4000-8000-000000000002',
  webhook_name: 'GitHub workflow runs',
  enabled: true,
  cause: 'verification',
  refusals: 42,
  reasons: { 'signature-mismatch': 41, 'signature-missing': 1 },
  refusing_since: '2026-09-18T04:57:00Z',
  refusing_secs: 97_200,
};


export const SAMPLE_CONDITION: EventCondition = {
  eventType: 'PullRequestReviewed',
  conditions: [{ repo: 'example-org/example-repo', pr: 412 }],
};


export function sampleConfirmDanger(): void {
  void showConfirm('Delete the trigger "Daily digest"? This cannot be undone.', 'Delete', {
    variant: 'danger',
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

export function sampleConsentPrompt(): void {
  void showConfirm(
    '"Nightly digest" wants to open the Files panel.',
    'Open',
    { title: 'A thread wants to navigate', cancelLabel: 'Not now', variant: 'default' },
  );
}


const INSTALL_FRAMES: AppUpdateRunning[] = [
  { version: '9.9.9', phase: 'checking' },
  { version: '9.9.9', phase: 'downloading', downloaded: 12_582_912, total: 68_157_440 },
  { version: '9.9.9', phase: 'downloading', downloaded: 43_646_976, total: 68_157_440 },
  { version: '9.9.9', phase: 'verifying' },
  { version: '9.9.9', phase: 'installing' },
  { version: '9.9.9', phase: 'restarting-services' },
  { version: '9.9.9', phase: 'relaunching' },
];

function sampleInstallDialog(frame: AppUpdateRunning): ProgressDialogState {
  return {
    ...appUpdateDialogState(frame, stopSampleProgressDialog),
    cancel: { label: 'Close preview', onClick: stopSampleProgressDialog },
  };
}

let ticker: ReturnType<typeof setInterval> | null = null;

export function stopSampleProgressDialog(): void {
  if (ticker !== null) {
    clearInterval(ticker);
    ticker = null;
  }
  progressDialog.value = { visible: false, title: '', message: '', progress: null };
}

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

export function showSampleProgressPhase(index: number): void {
  stopSampleProgressDialog();
  const frame = INSTALL_FRAMES[Math.min(Math.max(index, 0), INSTALL_FRAMES.length - 1)];
  progressDialog.value = sampleInstallDialog(frame);
}

export function sampleRestartDialog(): void {
  stopSampleProgressDialog();
  progressDialog.value = {
    ...restartDialogState(true),
    cancel: { label: 'Close preview', onClick: stopSampleProgressDialog },
  };
}
