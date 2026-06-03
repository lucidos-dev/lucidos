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
import type { EventSubscription, TriggerRun } from '../types';
import {
  listTriggers,
  listHistoricalTriggers,
  createTrigger,
  updateTrigger,
  deleteTriggerApi,
} from '../../api/client';
import { pushNavState } from './navigation';
import { setActiveMenu } from './menu';
import { revealContentPane } from './pane';
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

export function openEditTrigger(triggerId: string): void {
  panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId } };
  pushNavState();
}

export async function navigateToTrigger(triggerId: string): Promise<void> {
  if (triggers.value.status !== 'loaded') await loadTriggers();
  setActiveMenu('triggers', { type: 'form', form: { type: 'trigger', triggerId } });
  pushNavState();
  // Canonical helper: mobile swipe to content pane AND desktop expand of
  // collapsed split. The earlier `if (isMobile()) navigateToPane('content')`
  // only covered the mobile half — a trigger deep-link on a desktop with the
  // split collapsed silently looked like nothing happened.
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
  /** Event subscriptions to send. Caller is responsible for normalization
   *  (trimming, dropping blanks); the action sends the list as-is. Empty when
   *  the form is schedule-only. */
  on?: EventSubscription[];
  /** Whether the form is showing event fields — controls whether subscriptions
   *  are forwarded at all on update (false = explicitly clear). */
  showEvent?: boolean;
  /** When true, threads spawned by this trigger surface in REVIEW on completion. */
  goToReview: boolean;
  /** Group membership: undefined = leave unchanged (update only), null = clear,
   *  string = group id. Create requests treat undefined as "no group". */
  groupId?: string | null;
}

export async function submitTrigger(params: SubmitTriggerParams): Promise<boolean> {
  const { name, run, cronExpressions, triggerId, on, showEvent, goToReview, groupId } = params;
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

  try {
    if (triggerId) {
      const body: Parameters<typeof updateTrigger>[1] = {
        name: name.trim(),
        run,
        cron_expressions: trimmed,
        go_to_review: goToReview,
        // showEvent=false means the user moved to schedule-only; the empty list
        // clears any existing subscriptions on the backend.
        on: showEvent ? (on ?? []) : [],
      };
      // groupId: undefined = unchanged, null = clear, string = set. Same
      // triple-state semantics as the engine's app_id field.
      if (groupId !== undefined) body.group_id = groupId;
      const data = await updateTrigger(triggerId, body);
      if (!data.success) {
        showToast(data.error || 'Failed to update trigger', 'error');
        return false;
      }
    } else {
      const data = await createTrigger({
        name: name.trim(),
        run,
        cron_expressions: trimmed,
        on: on && on.length > 0 ? on : undefined,
        go_to_review: goToReview,
        // groupId on create: null and undefined both mean "no group"; only a
        // string sends a group_id to the engine.
        group_id: typeof groupId === 'string' ? groupId : undefined,
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

export async function deleteTrigger(
  triggerId: string,
  triggerName: string
): Promise<void> {
  if (!(await showConfirm(`Delete trigger "${triggerName}"?`))) {
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
