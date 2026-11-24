import {
  triggers,
  historicalTriggers,
  selectedTriggerIds,
  setSelectedTriggerIds,
  panelOverlay,
  triggerScrollTarget,
  closeInlineForm,
  showToast,
  showConfirm,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import type {
  EventSubscription,
  Loadable,
  TriggerInfo,
  TriggerRun,
} from '../types';
import type { ApiResult } from '../../api/types';
import {
  listTriggers,
  listHistoricalTriggers,
  createTrigger,
  updateTrigger,
  deleteTriggerApi,
  runTriggerApi,
} from '../../api/client';
import { pushNavState } from './navigation';
import { setActiveMenu } from './menu';
import { revealContentPane } from './pane';
import { errorDetail } from '../../utils/errorDetail';

export function pruneStaleSelectedTriggerIds(): void {
  if (triggers.value.status !== 'loaded' || historicalTriggers.value.status !== 'loaded') return;
  const valid = new Set<string>();
  for (const t of triggers.value.data) valid.add(t.id);
  for (const t of historicalTriggers.value.data) valid.add(t.id);
  const current = selectedTriggerIds.value;
  const next = new Set<string>();
  let dropped = 0;
  for (const id of current) {
    if (valid.has(id)) next.add(id);
    else dropped++;
  }
  if (dropped > 0) setSelectedTriggerIds(next);
}

export const triggersSlice: SliceOf<'triggers'> = {
  begin: () => setLoadingIfFresh(triggers),
  apply: (data) => {
    triggers.value = { status: 'loaded', data: data.triggers || [] };
    pruneStaleSelectedTriggerIds();
  },
  fail: (error) => { triggers.value = toFailed(error); },
};

export function loadTriggers(): Promise<void> {
  return loadSlice('triggers', triggersSlice, () => listTriggers());
}

export const historicalTriggersSlice: SliceOf<'historical_triggers'> = {
  begin: () => setLoadingIfFresh(historicalTriggers),
  apply: (data) => {
    historicalTriggers.value = { status: 'loaded', data: data.triggers || [] };
    pruneStaleSelectedTriggerIds();
  },
  fail: (error) => { historicalTriggers.value = toFailed(error); },
};

export function loadHistoricalTriggers(): Promise<void> {
  return loadSlice('historical_triggers', historicalTriggersSlice, () => listHistoricalTriggers());
}

export function openAddTrigger(): void {
  panelOverlay.value = { type: 'form', form: { type: 'trigger' } };
  pushNavState();
  revealContentPane();
}

export function openEditTrigger(triggerId: string): void {
  panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId } };
  pushNavState();
  revealContentPane();
}

function loadedWithout(list: Loadable<TriggerInfo[]>, triggerId: string): boolean {
  return list.status === 'loaded' && !list.data.some((t) => t.id === triggerId);
}

export async function navigateToTrigger(triggerId: string, source?: string): Promise<void> {
  const from = source ? ` (requested by ${source})` : '';
  const loadFailed = `Couldn't open trigger "${triggerId}"${from}: triggers failed to load`;

  if (triggers.value.status !== 'loaded') await loadTriggers();
  if (loadedWithout(triggers.value, triggerId)) {
    const snapshot = triggers.value;
    await loadTriggers();
    if (triggers.value.status === 'failed') {
      triggers.value = snapshot;
      showToast(loadFailed, 'error');
      return;
    }
    if (loadedWithout(triggers.value, triggerId)) {
      showToast(`Trigger "${triggerId}" no longer exists${from}`, 'error');
      return;
    }
  }
  if (triggers.value.status === 'failed') {
    showToast(loadFailed, 'error');
    return;
  }
  setActiveMenu('triggers');
  triggerScrollTarget.value = triggerId;
  pushNavState();
  revealContentPane();
}

export function closeTriggerForm(): void {
  closeInlineForm();
}

interface SubmitTriggerParams {
  name: string;
  run: TriggerRun;
  cronExpressions: string[];
  triggerId?: string;
  on?: EventSubscription[];
  showEvent?: boolean;
  goToReview: boolean;
  groupId?: string | null;
  codingAgentGrant: boolean;
  model: string | null;
  reasoningEffort: string | null;
  provider: string | null;
}

function surfaceWriteWarnings(result: ApiResult): void {
  for (const warning of [...(result.cron_preview?.warnings ?? []), ...(result.warnings ?? [])]) {
    showToast(warning, 'warning');
  }
}

export async function submitTrigger(params: SubmitTriggerParams): Promise<boolean> {
  const {
    name, run, cronExpressions, triggerId, on, showEvent, goToReview, groupId,
    codingAgentGrant, model, reasoningEffort, provider,
  } = params;
  if (!name.trim()) {
    showToast('Trigger name is required', 'error');
    return false;
  }
  const trimmed = cronExpressions.map(s => s.trim()).filter(Boolean);
  const hasOn = !!(on && on.length > 0);
  if (trimmed.length === 0 && !hasOn) {
    showToast('At least one cron expression or an event subscription is required', 'error');
    return false;
  }

  const isIntent = run.type === 'intent';
  const llmGoToReview = isIntent && goToReview;
  const llmModel = isIntent ? model : null;
  const llmEffort = isIntent ? reasoningEffort : null;
  const llmProvider = isIntent && llmModel ? provider : null;

  try {
    if (triggerId) {
      const body: Parameters<typeof updateTrigger>[1] = {
        name: name.trim(),
        run,
        cron_expressions: trimmed,
        go_to_review: llmGoToReview,
        on: showEvent ? (on ?? []) : [],
        coding_agent_grant: codingAgentGrant,
        model: llmModel,
        reasoning_effort: llmEffort,
        provider: llmProvider,
      };
      if (groupId !== undefined) body.group_id = groupId;
      const data = await updateTrigger(triggerId, body);
      if (!data.success) {
        showToast(data.error || 'Failed to update trigger', 'error');
        return false;
      }
      surfaceWriteWarnings(data);
    } else {
      const data = await createTrigger({
        name: name.trim(),
        run,
        cron_expressions: trimmed,
        on: on && on.length > 0 ? on : undefined,
        go_to_review: llmGoToReview,
        group_id: typeof groupId === 'string' ? groupId : undefined,
        coding_agent_grant: codingAgentGrant || undefined,
        model: llmModel ?? undefined,
        reasoning_effort: llmEffort ?? undefined,
        provider: llmProvider ?? undefined,
      });
      if (!data.success) {
        showToast(data.error || 'Failed to create trigger', 'error');
        return false;
      }
      surfaceWriteWarnings(data);
    }

    closeTriggerForm();
    await loadTriggers();
    return true;
  } catch (error) {
    showToast('Failed to save trigger: ' + errorDetail(error), 'error');
    return false;
  }
}

export async function toggleTrigger(
  triggerId: string,
  paused: boolean
): Promise<void> {
  try {
    const data = await updateTrigger(triggerId, { paused });
    if (data.success) {
      await loadTriggers();
    } else {
      showToast(data.error || 'Failed to update trigger', 'error');
    }
  } catch (error) {
    showToast('Failed to update trigger: ' + errorDetail(error), 'error');
  }
}

type TriggerEntry = { program: string } | { folder: string };

export function revokeTriggerGrant(triggerId: string, grant: TriggerEntry): Promise<void> {
  const body = 'program' in grant
    ? { revoke_program_grants: [grant.program] }
    : { revoke_folder_grants: [grant.folder] };
  return revokeTriggerEntry(triggerId, body, 'grant');
}

export function revokeTriggerDeny(triggerId: string, deny: TriggerEntry): Promise<void> {
  const body = 'program' in deny
    ? { revoke_program_denies: [deny.program] }
    : { revoke_folder_denies: [deny.folder] };
  return revokeTriggerEntry(triggerId, body, 'deny');
}

async function revokeTriggerEntry(
  triggerId: string,
  body: Parameters<typeof updateTrigger>[1],
  what: 'grant' | 'deny',
): Promise<void> {
  try {
    const data = await updateTrigger(triggerId, body);
    if (data.success) {
      await loadTriggers();
    } else {
      showToast(data.error || `Failed to revoke the ${what}`, 'error');
    }
  } catch (error) {
    showToast(`Failed to revoke the ${what}: ` + errorDetail(error), 'error');
  }
}

export async function runTriggerNow(triggerId: string): Promise<void> {
  try {
    const data = await runTriggerApi(triggerId);
    if (!data.success) {
      showToast(data.message || 'Failed to run trigger', 'error');
      return;
    }
    showToast(data.message, data.status === 'already-running' ? 'info' : 'success');
  } catch (error) {
    showToast('Failed to run trigger: ' + errorDetail(error), 'error');
  }
}

export async function deleteTrigger(
  triggerId: string,
  triggerName: string
): Promise<void> {
  if (!(await showConfirm(`Delete trigger "${triggerName}"?`, 'Delete', { variant: 'danger' }))) {
    return;
  }

  try {
    const data = await deleteTriggerApi(triggerId);
    if (data.success) {
      await loadTriggers();
    } else {
      showToast(data.error || 'Failed to delete trigger', 'error');
    }
  } catch (error) {
    showToast('Failed to delete trigger: ' + errorDetail(error), 'error');
  }
}
