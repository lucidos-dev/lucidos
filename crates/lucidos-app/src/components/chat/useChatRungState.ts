import {
  chatModels, effectiveThreadStatus, intelligenceLadderVersion, showToast, threadMap,
} from '../../store/store';
import {
  getComposeSelectionOverride, resolveModel, resolveProvider, resolveReasoningEffort,
} from '../../store/composeSelections';
import {
  resolveActiveThreadModel,
  resolveActiveThreadProvider,
  resolveActiveThreadReasoningEffort,
} from '../../store/threadModelSelections';
import { updateComposeSelection } from '../../store/actions/compose';
import {
  loadChatModels, lucidosModelChoices, LUCIDOS_TIER_VOCABULARY,
  PROVIDER_DEFAULT_EFFORT_LABEL, rememberProvider,
} from '../../store/actions/models';
import { accountChatEffort } from '../../store/actions/preferences';
import { ApiError, fetchSwitchEstimate, getChatLadder, recordChatSettings } from '../../api/client';
import { TURN_RUNNING_REASON } from '@lucidos/engine-constants';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { decodePair, encodePair, pairLabelOf } from '../../store/modelSelection';
import { displayModelName } from '../../store/thread-events/exchange';
import { openIntelligenceLadder } from '../../store/actions/menu';
import { errorDetail } from '../../utils/errorDetail';
import { providerStepOptions, providerStepCommit } from '../shared/ModelSelectionPicker';
import type { RungProviderStep } from './RungPicker';
import { rungFaces, rungLockedReason, stepTargets, type ShownRung } from './rungControlParts';
import { LADDER_LINK_VALUE, rungListRows } from './rungRows';
import { askRungSwitch } from './rungSwitch';

/** A model id with its context-window twin folded in. A rung names the bare
 *  model, so `claude-opus-5-5[1m]` runs the rung `claude-opus-5-5` names. */
function withoutContextTwin(model: string): string {
  return model.replace(/\[1m\]$/, '');
}

export interface ChatRungStateProps {
  /** The focused thread: the composing draft's id in compose, the active
   *  thread's otherwise. Absent on the fresh compose view. */
  threadId?: string;
  /** True for a composing draft and the fresh no-draft view. */
  composeContext: boolean;
}

/** Whether the engine refused a pick because a turn runs. */
function isTurnRunningRefusal(err: unknown): boolean {
  return err instanceof ApiError && (err.body as { reason?: unknown } | undefined)?.reason === TURN_RUNNING_REASON;
}

/** What the rung control knows about the Lucidos Agent thread it belongs to:
 *  the chat ladder's rungs, the pair in force, and how a pick commits.
 *
 *  A draft's pick is the draft's own override (or the pending slot before a
 *  draft exists). A started thread's pick is recorded on the engine, and only
 *  between turns: while a turn runs the rung is locked (ADR 0544). Neither
 *  reaches the account preference, so a pick never leaks to another draft or
 *  thread. */
export function useChatRungState({ threadId, composeContext: perDraft }: ChatRungStateProps) {
  // Idempotent render-path kick-off so the picker shows the live DB-backed
  // registry; `chatModelOptions()` falls back to the static MODELS list until
  // it lands. `loadChatModels` single-flights via setLoadingIfFresh.
  if (chatModels.value.status === 'not-loaded') void loadChatModels();

  /** Why a started thread's rung cannot change now, read at call time. */
  const lockedNow = (): string | null => {
    if (perDraft || !threadId) return null;
    const thread = threadMap.value.get(threadId);
    return rungLockedReason(thread ? effectiveThreadStatus(thread) : null);
  };
  const lockedReason = lockedNow();

  const modelValue = perDraft ? resolveModel(threadId) : resolveActiveThreadModel(threadId);
  const effortValue = perDraft
    ? resolveReasoningEffort(threadId)
    : resolveActiveThreadReasoningEffort(threadId, modelValue);
  // The tier another model would run at: the account's for a draft, and the
  // thread's own memory for that model on an active thread.
  const effortFor = (model: string) => (perDraft
    ? accountChatEffort(model)
    : resolveActiveThreadReasoningEffort(threadId, model));
  // A draft's pick belongs to the draft's model. A thread remembers one per
  // model, which the engine applies when the thread switches to it.
  const providerFor = (model: string) => (perDraft
    ? (model === modelValue ? resolveProvider(threadId) : null)
    : resolveActiveThreadProvider(threadId, model));

  /** A draft's pick lands on the draft. The backend binds here too, and is
   *  ALSO remembered on the model's row, since the last provider picked for a
   *  model is remembered per model. */
  function applyDraftPick(patch: ModelSelectionPatch) {
    updateComposeSelection(threadId ?? null, {
      model: patch.model,
      // `undefined` drops this draft's own tier.
      reasoningEffort: patch.reasoningEffort ?? undefined,
      // `undefined` clears a pick the new model cannot use.
      provider: patch.provider ?? undefined,
    });
    if (patch.provider) void rememberProvider(patch.model, patch.provider);
  }

  const selection = useModelSelection({
    models: lucidosModelChoices(modelValue),
    vocabulary: LUCIDOS_TIER_VOCABULARY,
    model: modelValue,
    effort: effortValue,
    providerFor,
    effortFor,
    unsetEffortLabel: PROVIDER_DEFAULT_EFFORT_LABEL,
    onChange: applyDraftPick,
  });

  const ladder = useLoadableFetch(getChatLadder, [intelligenceLadderVersion.value], {
    keepLoadedWhileRefetching: true,
  }).loadable;
  const served = ladder.status === 'loaded' ? ladder.data : null;
  const servedRungs = served?.rungs ?? [];
  const defaultRung = served?.default_rung ?? null;
  // Only the rungs this workspace can route, as the model picker listed them.
  const routable = new Set(selection.rows.map((r) => withoutContextTwin(r.value)));
  const onRungs = servedRungs.filter((r) => r.on && routable.has(withoutContextTwin(r.model)));
  const labelOf = (model: string) => selection.rows.find((r) => r.value === model)?.label ?? displayModelName(model);
  // A draft that picked nothing follows the account default, which is the
  // default rung. A started thread always has a pair of its own.
  const override = perDraft ? getComposeSelectionOverride(threadId) : null;
  const followsDefault = override !== null && override.model === undefined && override.reasoningEffort === undefined;

  const rungList = rungListRows({
    rungs: onRungs,
    options: [],
    model: followsDefault && defaultRung ? null : modelValue,
    effort: selection.effort,
    modelLabel: labelOf,
    defaultPair: perDraft ? defaultRung : null,
    opensStep: (model) => (selection.rows.find((r) => r.value === runModelOf(model))?.providers.length ?? 0) > 0,
  });

  // The model's own label, so a 1M context window stays named on the face.
  const at = onRungs.findIndex((r) => encodePair(r.model, r.effort) === rungList.current);
  const shown: ShownRung = {
    name: labelOf(modelValue),
    effort: selection.effort,
    position: at === -1 ? null : { at: at + 1, of: onRungs.length },
  };
  // A rung can run its model with or without the 1M context window, so it
  // reserves both labels whichever model is in force now.
  const faces = rungFaces(onRungs, (rung) => {
    const twins = selection.rows.filter((r) => withoutContextTwin(r.value) === withoutContextTwin(rung.model));
    return twins.length > 0 ? twins.map((r) => r.label) : [labelOf(rung.model)];
  });

  /** The model a rung's pick runs. A rung names the bare model, so picking the
   *  model in force keeps its context window: the window is part of the
   *  prompt cache's key. */
  function runModelOf(model: string): string {
    return withoutContextTwin(model) === withoutContextTwin(modelValue) ? modelValue : model;
  }

  /** The pair a rung row commits, with the backend when one was chosen. The
   *  backend's own efforts snap the rung's effort, so no request is refused. */
  function rungPick(value: string, provider?: string): { encoded: string; provider?: string } {
    const pair = decodePair(value);
    const model = runModelOf(pair.model);
    const row = selection.rows.find((r) => r.value === model);
    if (provider && row) return { encoded: providerStepCommit(row, provider, pair.effort), provider };
    return { encoded: encodePair(model, pair.effort) };
  }

  /** The backends a rung opens, the preferred-provider step. */
  function providerStep(value: string): RungProviderStep | null {
    if (value === LADDER_LINK_VALUE) return null;
    const row = selection.rows.find((r) => r.value === runModelOf(decodePair(value).model));
    if (!row || row.providers.length === 0) return null;
    return {
      label: `${row.label} · Provider`,
      options: providerStepOptions(row),
      current: row.value === modelValue ? row.provider : null,
    };
  }

  /** Whether a pick on a started thread goes ahead. The pair already in force
   *  is silent. Any other asks the engine whether it rebuilds a warm cache, and
   *  only then asks the user. A chat thread has no successor to hand off to, so
   *  the choice is to keep the pair or switch anyway. */
  async function startedPickProceeds(
    startedThreadId: string,
    encoded: string,
    provider: string | undefined,
    label: string,
  ): Promise<boolean> {
    if (encoded === selection.value && (provider ?? selection.provider) === selection.provider) return true;
    const pair = decodePair(encoded);
    let estimate;
    try {
      estimate = await fetchSwitchEstimate(startedThreadId, pair.model, pair.effort);
    } catch (err) {
      showToast(`Could not check what switching to ${label} costs: ${errorDetail(err)}`, 'error');
      return false;
    }
    if (!estimate.rebuilds_cache) return true;
    const choice = await askRungSwitch({
      from: selection.label,
      to: label,
      extraCostUsd: estimate.extra_cost_usd,
      canHandOff: false,
    });
    return choice === 'switch';
  }

  /** Record a started thread's pick on the engine. A turn may start while the
   *  user reads the switch dialog, so the lock is checked again after it. */
  async function recordStartedPick(startedThreadId: string, encoded: string, provider: string | undefined) {
    const label = pairLabelOf(selection.rows, encoded, provider);
    if (!await startedPickProceeds(startedThreadId, encoded, provider, label)) return false;
    const locked = lockedNow();
    if (locked) {
      showToast(locked, 'info');
      return false;
    }
    const pair = decodePair(encoded);
    try {
      await recordChatSettings(startedThreadId, {
        model: pair.model,
        reasoningEffort: pair.effort ?? undefined,
        provider,
      });
    } catch (err) {
      if (isTurnRunningRefusal(err)) showToast(rungLockedReason('running')!, 'info');
      else showToast(`Could not switch to ${label}: ${errorDetail(err)}`, 'error');
      return false;
    }
    if (provider) void rememberProvider(pair.model, provider);
    return true;
  }

  async function pick(encoded: string, provider?: string, quiet = false) {
    const label = pairLabelOf(selection.rows, encoded, provider);
    if (perDraft || !threadId) selection.pick(encoded, provider);
    else if (!await recordStartedPick(threadId, encoded, provider)) return;
    if (!quiet) showToast(`Model: ${label}`, 'success');
  }

  /** Commit a row: a rung, or the ladder link out to Settings > Intelligence. */
  function pickRung(value: string, provider?: string) {
    if (value === LADDER_LINK_VALUE) {
      openIntelligenceLadder('chat');
      return;
    }
    const commit = rungPick(value, provider);
    void pick(commit.encoded, commit.provider);
  }

  const steps = stepTargets(rungList.rows, rungList.current);

  /** Move one row up (-1) or down (1) the rung list, saying nothing more: the
   *  label shows the new pair. */
  function step(direction: -1 | 1) {
    const locked = lockedNow();
    if (locked) {
      showToast(locked, 'info');
      return;
    }
    const target = direction === -1 ? steps.prev : steps.next;
    if (target === null) return;
    const commit = rungPick(target);
    void pick(commit.encoded, undefined, true);
  }

  return { shown, faces, rungList, providerStep, pickRung, steps, step, lockedReason };
}
