import {
  triggers,
  historicalTriggers,
  selectedTriggerIds,
  setSelectedTriggerIds,
  panelOverlay,
  closeInlineForm,
  showToast,
  showConfirm,
} from '../store';
import { toFailed, setLoadingIfFresh } from '../types';
import type { TriggerRun } from '../types';
import {
  listTriggers,
  listHistoricalTriggers,
  createTrigger,
  updateTrigger,
  deleteTriggerApi,
} from '../../api/client';
import { pushNavState } from './navigation';
import { setActiveMenu } from './menu';
import { navigateToPane } from './pane';
import { isMobile } from '../../utils/viewport';
import { errorDetail } from '../../utils/errorDetail';

/** Drop selectedTriggerIds entries that aren't in either registry. No-ops
 *  until both registries are loaded so a still-fetching list doesn't drop
 *  the user's selection. */
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

export async function loadTriggers(): Promise<void> {
  setLoadingIfFresh(triggers);
  try {
    const data = await listTriggers();
    triggers.value = { status: 'loaded', data: data.triggers || [] };
    pruneStaleSelectedTriggerIds();
  } catch (error) {
    triggers.value = toFailed(error);
  }
}

export async function loadHistoricalTriggers(): Promise<void> {
  setLoadingIfFresh(historicalTriggers);
  try {
    const data = await listHistoricalTriggers();
    historicalTriggers.value = { status: 'loaded', data: data.triggers || [] };
    pruneStaleSelectedTriggerIds();
  } catch (error) {
    historicalTriggers.value = toFailed(error);
  }
}

export function openAddTrigger(): void {
  panelOverlay.value = { type: 'form', form: { type: 'trigger' } };
  pushNavState();
}

export function openEditTrigger(taskId: string): void {
  panelOverlay.value = { type: 'form', form: { type: 'trigger', taskId } };
  pushNavState();
}

export async function navigateToTrigger(taskId: string): Promise<void> {
  if (triggers.value.status !== 'loaded') await loadTriggers();
  setActiveMenu('triggers', { type: 'form', form: { type: 'trigger', taskId } });
  // Always go to the content pane on mobile — setActiveMenu's pane nav only
  // fires when switching menus from the chat pane, but a deep link should
  // surface the form regardless of where the user was.
  if (isMobile()) navigateToPane('content');
  pushNavState();
}

export function closeTriggerForm(): void {
  closeInlineForm();
}

interface SubmitTriggerParams {
  name: string;
  run: TriggerRun;
  cronExpressions: string[];
  taskId?: string;
  onEvent?: string;
  condition?: Record<string, unknown>;
  /** Whether the form is showing event fields — controls whether on_event/condition are sent on update. */
  showEvent?: boolean;
  /** When true, threads spawned by this trigger surface in REVIEW on completion. */
  goToReview: boolean;
}

export async function submitTrigger(params: SubmitTriggerParams): Promise<boolean> {
  const { name, run, cronExpressions, taskId, onEvent, condition, showEvent, goToReview } = params;
  if (!name.trim()) {
    showToast('Trigger name is required', 'error');
    return false;
  }
  const trimmed = cronExpressions.map(s => s.trim()).filter(Boolean);
  if (trimmed.length === 0 && !onEvent) {
    showToast('At least one cron expression or an event type is required', 'error');
    return false;
  }

  try {
    if (taskId) {
      const body: Parameters<typeof updateTrigger>[1] = {
        name: name.trim(),
        run,
        cron_expressions: trimmed,
        go_to_review: goToReview,
      };
      if (showEvent) {
        body.on_event = onEvent || null;
        body.condition = condition || null;
      } else {
        // User switched to schedule-only — explicitly clear event fields
        body.on_event = null;
        body.condition = null;
      }
      const data = await updateTrigger(taskId, body);
      if (!data.success) {
        showToast(data.error || 'Failed to update trigger', 'error');
        return false;
      }
    } else {
      const data = await createTrigger({
        name: name.trim(),
        run,
        cron_expressions: trimmed,
        on_event: onEvent,
        condition,
        go_to_review: goToReview,
      });
      if (!data.success) {
        showToast(data.error || 'Failed to create trigger', 'error');
        return false;
      }
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
  taskId: string,
  paused: boolean
): Promise<void> {
  try {
    const data = await updateTrigger(taskId, { paused });
    if (data.success) {
      await loadTriggers();
    } else {
      showToast(data.error || 'Failed to update trigger', 'error');
    }
  } catch (error) {
    showToast('Failed to update trigger: ' + errorDetail(error), 'error');
  }
}

export async function deleteTrigger(
  taskId: string,
  taskName: string
): Promise<void> {
  if (!(await showConfirm(`Delete trigger "${taskName}"?`))) {
    return;
  }

  try {
    const data = await deleteTriggerApi(taskId);
    if (data.success) {
      await loadTriggers();
    } else {
      showToast(data.error || 'Failed to delete trigger', 'error');
    }
  } catch (error) {
    showToast('Failed to delete trigger: ' + errorDetail(error), 'error');
  }
}
