
import { signal } from '@preact/signals';
import type { EmbeddingModelStatus } from '../api/types';
import type { TailscaleServeProgress } from '../utils/tauri';
import type { CommitGroup, CommitGroupKind, PendingCommits } from '../api/client';
import { formatBytes } from '../utils/formatBytes';
import { formatElapsed } from '../utils/elapsed';

export const embeddingModelStatus = signal<EmbeddingModelStatus | null>(null);

export const tailscaleServeRun = signal<TailscaleServeProgress | null>(null);

export interface ElapsedAnchor {
  elapsedMs: number | null;
  anchoredAt: number;
}

export interface EngineBuildDetail extends ElapsedAnchor {
  verifying?: boolean;
  pendingCommits: PendingCommits | null;
  queuedBehind?: readonly string[];
}

export const engineBuildDetail = signal<EngineBuildDetail | null>(null);

export const frontendRefreshDetail = signal<ElapsedAnchor | null>(null);

export type ActivityAction =
  | { kind: 'open-url'; label: string; url: string }
  | { kind: 'cancel-tailscale-serve'; label: string };

export interface BackgroundActivity {
  kind: 'engine-build' | 'frontend-refresh' | 'embedding-model' | 'tailscale-serve';
  label: string;
  detail?: string;
  progress: number | null;
  note?: string;
  queued?: string;
  action?: ActivityAction;
  secondaryAction?: ActivityAction;
}

export const MEMORY_NOT_INDEXED_NOTE =
  'You can keep working. Anything created before this finishes will not be searchable in memory.';

export function backgroundActivities(
  engineBuilding: boolean,
  model: EmbeddingModelStatus | null,
  serveRun: TailscaleServeProgress | null = null,
  buildDetail: EngineBuildDetail | null = null,
  nowMs: number = Date.now(),
  frontendRefresh: ElapsedAnchor | null = null,
): BackgroundActivity[] {
  const activities: BackgroundActivity[] = [];
  if (engineBuilding) {
    const queuedBehind = buildDetail?.queuedBehind;
    activities.push({
      kind: 'engine-build',
      label: buildDetail?.verifying ? 'Verifying new version' : queuedBehind ? 'New version queued' : 'Building new version',
      progress: null,
      detail: buildElapsedDetail(buildDetail, nowMs),
      note: 'You can switch to it once the build finishes.',
      queued: queuedBehind ? slotWaitReason(queuedBehind) : undefined,
    });
  }
  if (frontendRefresh) {
    activities.push({
      kind: 'frontend-refresh',
      label: 'Building frontend',
      progress: null,
      detail: buildElapsedDetail(frontendRefresh, nowMs),
      note: 'A Refresh prompt appears once the build finishes.',
    });
  }
  const state = model?.load_state;
  if (state?.kind === 'downloading') {
    activities.push({
      kind: 'embedding-model',
      label: 'Downloading embedding model',
      detail: downloadDetail(state.downloaded_bytes, state.total_bytes),
      progress: downloadFraction(state.downloaded_bytes, state.total_bytes),
      note: MEMORY_NOT_INDEXED_NOTE,
    });
  }
  const serve = tailscaleServeActivity(serveRun);
  if (serve) activities.push(serve);
  return activities;
}

export function tailscaleServeActivity(
  run: TailscaleServeProgress | null,
): BackgroundActivity | null {
  if (!run) return null;
  const base = { kind: 'tailscale-serve', progress: null } as const;
  const cancel: ActivityAction = { kind: 'cancel-tailscale-serve', label: 'Cancel' };
  switch (run.phase) {
    case 'starting':
    case 'checking-tailnet':
      return { ...base, label: 'Setting up Tailscale access', secondaryAction: cancel };
    case 'configuring':
      return { ...base, label: 'Configuring tailscale serve', secondaryAction: cancel };
    case 'awaiting-tailnet-approval':
      return {
        ...base,
        label: 'Waiting for you to enable Serve on your tailnet',
        note:
          'Tailscale needs Serve turned on for your tailnet before it can give this Mac an ' +
          'HTTPS address. Open the link, approve it, and setup continues on its own.',
        action: { kind: 'open-url', label: 'Enable in Tailscale', url: run.url },
        secondaryAction: cancel,
      };
    case 'waiting-for-https':
      return {
        ...base,
        label: 'Waiting for HTTPS to come up',
        note: 'The first certificate for this name can take a few seconds to provision.',
        secondaryAction: cancel,
      };
    case 'done':
    case 'failed':
    case 'cancelled':
      return null;
  }
}

function slotWaitReason(holders: readonly string[]): string {
  const list = holders.join(', ');
  return holders.length === 1
    ? `The one build slot is busy: ${list}. It takes it as soon as it frees up.`
    : `All ${holders.length} build slots are busy: ${list}. It takes the next one that frees up.`;
}

function buildElapsedDetail(
  detail: ElapsedAnchor | null,
  nowMs: number,
): string | undefined {
  if (detail?.elapsedMs == null) return undefined;
  return formatElapsed(detail.elapsedMs + Math.max(0, nowMs - detail.anchoredAt));
}

export const GROUP_LABEL: Record<Exclude<CommitGroupKind, 'housekeeping'>, string> = {
  new: 'New',
  fixed: 'Fixed',
  improved: 'Improved',
  other: 'Other',
};

export function describedItems(group: CommitGroup): string[] {
  const hidden = group.total - group.descriptions.length;
  return hidden > 0 ? [...group.descriptions, `and ${hidden} more`] : [...group.descriptions];
}

export function pendingCommitsHeadline(total: number): string {
  return `${total === 1 ? '1 commit' : `${total} commits`} ${comeWithTheNewVersion(total)}`;
}

export function comeWithTheNewVersion(total: number): string {
  return total === 1 ? 'comes with the new version' : 'come with the new version';
}

export function housekeepingLine(total: number): string {
  return total === 1
    ? '1 housekeeping commit (docs, tests, chores)'
    : `${total} housekeeping commits (docs, tests, chores)`;
}

function downloadDetail(downloaded: number, total: number): string {
  return total > 0
    ? `${formatBytes(downloaded)} of ${formatBytes(total)}`
    : formatBytes(downloaded);
}

function downloadFraction(downloaded: number, total: number): number | null {
  if (!(total > 0) || !Number.isFinite(downloaded)) return null;
  return Math.min(1, Math.max(0, downloaded / total));
}

export function embeddingModelOutcome(
  state: EmbeddingModelStatus['load_state'] | undefined,
): { title?: string; message: string; tone: 'success' | 'warning' | 'error' } | null {
  switch (state?.kind) {
    case 'failed':
      return { title: 'Embedding model unavailable', message: state.message, tone: 'error' };
    case 'waiting':
      return {
        title: 'Waiting to download the embedding model',
        message:
          'The download has not succeeded yet, so memory search and extraction are off. ' +
          'Lucidos keeps retrying in the background.',
        tone: 'warning',
      };
    case 'ready':
      return {
        message: 'Embedding model ready. Everything you create from now on is searchable in memory.',
        tone: 'success',
      };
    default:
      return null;
  }
}

export function tailscaleServeOutcome(
  run: TailscaleServeProgress | null,
): { message: string; tone: 'success' | 'error' } | null {
  switch (run?.phase) {
    case 'done':
      return {
        message: `Lucidos is now reachable over Tailscale at ${run.url}`,
        tone: 'success',
      };
    case 'failed':
      return { message: run.message, tone: 'error' };
    default:
      return null;
  }
}
