import {
  triggers,
  panelOverlay,
  closeInlineForm,
  showToast,
  showConfirm,
} from '../store';
import { toFailed } from '../types';
import type { TriggerRun } from '../types';
import {
  listTriggers,
  createTrigger,
  updateTrigger,
  deleteTriggerApi,
} from '../../api/client';
import { pushNavState } from './navigation';
import { setActiveMenu } from './menu';
import { errorDetail } from '../../utils/errorDetail';

export async function loadTriggers(): Promise<void> {
  if (triggers.value.status !== 'loaded') {
    triggers.value = { status: 'loading' };
  }
  try {
    const data = await listTriggers();
    triggers.value = { status: 'loaded', data: data.triggers || [] };
  } catch (error) {
    console.error('Failed to load triggers:', error);
    triggers.value = toFailed(error);
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
  setActiveMenu('triggers');
  await loadTriggers();
  openEditTrigger(taskId);
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
}

export async function submitTrigger(params: SubmitTriggerParams): Promise<boolean> {
  const { name, run, cronExpressions, taskId, onEvent, condition, showEvent } = params;
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
    console.error('Failed to save trigger:', error);
    showToast('Failed to save trigger: ' + errorDetail(error), 'error');
    return false;
  }
}

export async function toggleTrigger(
  taskId: string,
  enabled: boolean
): Promise<void> {
  try {
    const data = await updateTrigger(taskId, { enabled });
    if (data.success) {
      await loadTriggers();
    } else {
      showToast(data.error || 'Failed to update trigger', 'error');
    }
  } catch (error) {
    console.error('Failed to toggle trigger:', error);
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
    console.error('Failed to delete trigger:', error);
    showToast('Failed to delete trigger: ' + errorDetail(error), 'error');
  }
}
