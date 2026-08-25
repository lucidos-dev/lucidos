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
import type { EventSubscription, SideEffectCategory, TriggerRun } from '../types';
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
  // Lands the new-trigger form in the content pane: mobile swipe + desktop
  // split expand. Mirrors openEditTrigger (same view, same overlay surface) —
  // without it a click on a collapsed-split desktop silently looks like a no-op.
  revealContentPane();
}

export function openEditTrigger(triggerId: string): void {
  panelOverlay.value = { type: 'form', form: { type: 'trigger', triggerId } };
  pushNavState();
  // Lands the edit form in the content pane: mobile swipe + desktop split
  // expand. Now the whole trigger row's click handler, so it must reveal the
  // pane like openApp does for the apps panel.
  revealContentPane();
}

/** Go to a trigger: the Triggers panel, scrolled to that trigger's ROW and
 *  marked with the navigation focus marker.
 *
 *  The row rather than the edit form, because the row is where every
 *  affordance a "here is your trigger" pointer means lives: Run once, the
 *  pause toggle, the last-run OK or failed chip, the schedule. The form is for
 *  changing the configuration, and the row is one tap from it. See
 *  ADR 0112 and docs/plans/2026-08-24-a-trigger-is-a-link.md.
 *
 *  Every route to a trigger comes through here, so they all land the same way:
 *  a `trigger:<id>` chat link, a notification tap, the notification's Open
 *  trigger button, a Search Everywhere hit, `navigate_ui`. */
export async function navigateToTrigger(triggerId: string, source?: string): Promise<void> {
  // `source` names where the navigate originated (e.g. a thread label) so a
  // genuine-miss toast says where it came from instead of swallowing it.
  const from = source ? ` (requested by ${source})` : '';

  // Defense-in-depth on a cache miss, mirroring openAppById: `triggers` is a
  // cached projection refreshed by Trigger* SSE events, so a sibling thread
  // that just created the trigger leaves it momentarily stale. Re-fetch the
  // source of truth before concluding the trigger is gone — a stale-cache
  // pre-check would report a live trigger as "no longer exists" and swallow
  // the real cause.
  if (triggers.value.status !== 'loaded') await loadTriggers();
  if (triggers.value.status === 'loaded' && !triggers.value.data.some((t) => t.id === triggerId)) {
    await loadTriggers();
    if (triggers.value.status === 'loaded' && !triggers.value.data.some((t) => t.id === triggerId)) {
      showToast(`Trigger "${triggerId}" no longer exists${from}`, 'error');
      return;
    }
  }
  if (triggers.value.status === 'failed') {
    // loadTriggers stamped the failure on the Loadable, but the user who
    // clicked the link isn't on the triggers panel — surface it directly.
    showToast(`Couldn't open trigger "${triggerId}"${from} — triggers failed to load`, 'error');
    return;
  }
  // A null overlay, so an edit form already open on another trigger closes and
  // the list itself is what shows. `TriggersView`'s effect consumes the target
  // once the rows render, expanding the trigger's group first if it is
  // collapsed.
  setActiveMenu('triggers');
  triggerScrollTarget.value = triggerId;
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
  /** Side-effect grant (ADR 0002, Phase 5) — the full set this trigger is
   *  authorized to perform unattended. Always sent as a complete list (the
   *  engine replaces wholesale); `[]` clears all grants. */
  sideEffectGrant: SideEffectCategory[];
  /** The *trigger model* this trigger's intent fires on; `null` = the account
   *  default (Settings → Models → Chat & triggers). Sent on every update so
   *  switching back to Default clears the stored pin. */
  model: string | null;
  /** Thinking budget for this trigger's intent fires; `null` = the account
   *  default. Same send semantics as `model`. */
  reasoningEffort: string | null;
}

/** Surface the engine's non-fatal advice after a successful save: the cron
 *  warnings, then the event-type ones.
 *
 *  Warnings only, in both families. A schedule that can never fire, and an
 *  event type the engine never emits, are both rejected outright and arrive as
 *  `error`. The next-run preview needs no toast, since the reload below renders
 *  it on the trigger's own row. */
function surfaceWriteWarnings(result: ApiResult): void {
  for (const warning of [...(result.cron_preview?.warnings ?? []), ...(result.warnings ?? [])]) {
    showToast(warning, 'warning');
  }
}

export async function submitTrigger(params: SubmitTriggerParams): Promise<boolean> {
  const {
    name, run, cronExpressions, triggerId, on, showEvent, goToReview, groupId, sideEffectGrant,
    model, reasoningEffort,
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

  // Four fields apply only to the intent (LLM) path: a script trigger runs no
  // LLM and reads none of them. Gate here, once, so a caller can never persist
  // state left over from an intent → script switch, and so the rule lives in
  // one place instead of four ternaries in the form.
  const isIntent = run.type === 'intent';
  const llmGoToReview = isIntent && goToReview;
  const llmGrant = isIntent ? sideEffectGrant : [];
  const llmModel = isIntent ? model : null;
  const llmEffort = isIntent ? reasoningEffort : null;

  try {
    if (triggerId) {
      const body: Parameters<typeof updateTrigger>[1] = {
        name: name.trim(),
        run,
        cron_expressions: trimmed,
        go_to_review: llmGoToReview,
        // showEvent=false means the user moved to schedule-only; the empty list
        // clears any existing subscriptions on the backend.
        on: showEvent ? (on ?? []) : [],
        // Full replacement every save (the form always reflects the complete
        // grant); `[]` clears all grants.
        side_effect_grant: llmGrant,
        // Always sent, never omitted: `null` is how "back to Default" reaches
        // the engine, and omitting the field would leave a stale pin in place.
        model: llmModel,
        reasoning_effort: llmEffort,
      };
      // groupId: undefined = unchanged, null = clear, string = set. Same
      // triple-state semantics as the engine's app_id field.
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
        // groupId on create: null and undefined both mean "no group"; only a
        // string sends a group_id to the engine.
        group_id: typeof groupId === 'string' ? groupId : undefined,
        // Only send a grant when non-empty (keeps the create payload clean for
        // the common no-grant trigger).
        side_effect_grant: llmGrant.length > 0 ? llmGrant : undefined,
        // Omitted on create when Default: there is no stored pin to clear yet,
        // so a new trigger on the account defaults keeps a clean payload.
        model: llmModel ?? undefined,
        reasoning_effort: llmEffort ?? undefined,
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

/** Fire an existing trigger once, off-schedule.
 *
 *  The toast reports what actually happened rather than assuming a start:
 *  `already-running` means admission coalesced the fire away and nothing new
 *  began, so it toasts as info, not success.
 *
 *  Deliberately does NOT reload the list. Nothing on the row changes at submit
 *  time, and when the run records, the `TriggerExecuted` SSE arm in
 *  `entityReferences.ts` already calls `loadTriggers`. */
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
