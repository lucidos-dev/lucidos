
import {
  showToast,
  dismissToast,
  toasts,
  engineBuilding,
  engineBuildDetail,
  frontendRefreshDetail,
  embeddingModelStatus,
  tailscaleServeRun,
} from '../store';
import {
  embeddingModelOutcome,
  tailscaleServeActivity,
  tailscaleServeOutcome,
  type ActivityAction,
} from '../backgroundActivity';
import { getEmbeddingModelStatus } from '../../api/client';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import type { EmbeddingModelStatus } from '../../api/types';
import { isTauri } from '../../utils/platform';
import { openExternalUrl } from '../../utils/openExternalUrl';
import {
  listen,
  openExternal,
  cancelTailscaleServe,
  TAILSCALE_SERVE_PROGRESS_EVENT,
  type TailscaleServeProgress,
} from '../../utils/tauri';

export const SERVE_RUN_TOAST_KEY = 'tailscale-serve';

export const SERVE_OUTCOME_TOAST_KEY = 'tailscale-serve-outcome';

export const EMBEDDING_OUTCOME_TOAST_KEY = 'embedding-model-outcome';

const SETTLED_DISMISS_MS = 8000;

let downloadSeen = false;

let announcedOutcome: EmbeddingModelStatus['load_state']['kind'] | null = null;

let liveVersion = 0;

let unlistenServe: (() => void) | null = null;
let subscribingServe = false;

export function resetBackgroundActivityForTest(): void {
  downloadSeen = false;
  announcedOutcome = null;
  liveVersion = 0;
}

export function applyEmbeddingModelStatus(status: EmbeddingModelStatus): void {
  liveVersion += 1;
  embeddingModelStatus.value = status;
  syncEmbeddingModelOutcome();
}

export function syncEmbeddingModelOutcome(): void {
  const state = embeddingModelStatus.value?.load_state;
  if (state?.kind === 'downloading') {
    downloadSeen = true;
    announcedOutcome = null;
    return;
  }
  if (!downloadSeen || !state || state.kind === announcedOutcome) return;
  const outcome = embeddingModelOutcome(state);
  if (!outcome) return;
  showToast(outcome.message, outcome.tone, {
    title: outcome.title,
    key: EMBEDDING_OUTCOME_TOAST_KEY,
    autoDismissMs: outcome.tone === 'error' ? undefined : SETTLED_DISMISS_MS,
  });
  if (toasts.value.some((t) => t.key === EMBEDDING_OUTCOME_TOAST_KEY)) announcedOutcome = state.kind;
}

export function activityAction(action: ActivityAction | undefined) {
  if (!action) return undefined;
  switch (action.kind) {
    case 'open-url': {
      const { url } = action;
      return {
        label: action.label,
        onClick: () => {
          if (!isTauri()) {
            openExternalUrl(url);
            return;
          }
          openExternal(url).catch((e) => {
            showToast(`Couldn't open ${url}: ${String(e)}`, 'error');
          });
        },
      };
    }
    case 'cancel-tailscale-serve':
      return {
        label: action.label,
        onClick: () => {
          void cancelTailscaleServe().catch((e) => {
            showToast(`Couldn't cancel: ${String(e)}`, 'error');
          });
        },
      };
  }
}

export const embeddingModelStatusSlice: SliceOf<'embedding_model_status', number> = {
  begin: () => liveVersion,
  apply: (status, readAt) => {
    if (readAt !== liveVersion) return;
    embeddingModelStatus.value = status;
    syncEmbeddingModelOutcome();
  },
  fail: (e) => {
    console.warn('[background-activity] embedding-model status read failed', e);
  },
};

export function loadEmbeddingModelStatus(): Promise<void> {
  return loadSlice('embedding_model_status', embeddingModelStatusSlice, () => getEmbeddingModelStatus());
}

export function buildTimerIsLive(): boolean {
  const engineTicks = engineBuilding.value && engineBuildDetail.value?.elapsedMs != null;
  return engineTicks || frontendRefreshDetail.value?.elapsedMs != null;
}


function serveToastIsOpen(): boolean {
  return toasts.value.some((t) => t.key === SERVE_RUN_TOAST_KEY);
}

function renderServeRun(): void {
  const activity = tailscaleServeActivity(tailscaleServeRun.value);
  if (!activity) {
    dismissToast(SERVE_RUN_TOAST_KEY);
    return;
  }
  const [title, message] = activity.note ? [activity.label, activity.note] : [undefined, activity.label];
  showToast(message, 'info', {
    title,
    key: SERVE_RUN_TOAST_KEY,
    spinning: true,
    action: activityAction(activity.action),
    secondaryAction: activityAction(activity.secondaryAction),
  });
}

export function beginTailscaleServeRun(): void {
  tailscaleServeRun.value = { phase: 'starting' };
  renderServeRun();
}

export function clearTailscaleServeRun(): void {
  tailscaleServeRun.value = null;
  dismissToast(SERVE_RUN_TOAST_KEY);
}

export function applyTailscaleServeProgress(frame: TailscaleServeProgress): void {
  if (frame.phase === 'done' || frame.phase === 'failed' || frame.phase === 'cancelled') {
    clearTailscaleServeRun();
    const outcome = tailscaleServeOutcome(frame);
    if (outcome) {
      showToast(outcome.message, outcome.tone, {
        key: SERVE_OUTCOME_TOAST_KEY,
        autoDismissMs: outcome.tone === 'error' ? undefined : SETTLED_DISMISS_MS,
      });
    }
    return;
  }
  tailscaleServeRun.value = frame;
  if (serveToastIsOpen()) renderServeRun();
}

export async function subscribeToTailscaleServeProgress(): Promise<void> {
  if (!isTauri() || unlistenServe || subscribingServe) return;
  subscribingServe = true;
  try {
    unlistenServe = await listen<TailscaleServeProgress>(
      TAILSCALE_SERVE_PROGRESS_EVENT,
      (e) => { applyTailscaleServeProgress(e.payload); },
    );
  } catch (e) {
    console.warn('[background-activity] tailscale serve progress subscription failed', e);
  } finally {
    subscribingServe = false;
  }
}

export function unsubscribeFromTailscaleServeProgress(): void {
  if (unlistenServe) {
    unlistenServe();
    unlistenServe = null;
  }
}
