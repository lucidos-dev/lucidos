import { useEffect } from 'preact/hooks';
import {
  chatLadder, chatModels, effectiveThreadStatus, showToast, threadMap,
} from '../../store/store';
import {
  getComposeSelectionOverride, resolveModel, resolveProvider, resolveReasoningEffort,
} from '../../store/composeSelections';
import {
  chatTurnSelection,
  resolveActiveThreadModel,
  resolveActiveThreadProvider,
  resolveActiveThreadReasoningEffort,
} from '../../store/threadModelSelections';
import { updateComposeSelection } from '../../store/actions/compose';
import {
  loadChatLadder, loadChatModels, lucidosModelChoices, LUCIDOS_TIER_VOCABULARY,
  PROVIDER_DEFAULT_EFFORT_LABEL, rememberProvider,
} from '../../store/actions/models';
import { accountChatEffort } from '../../store/actions/preferences';
import { fetchSwitchEstimate, recordChatSettings } from '../../api/client';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';
import { decodePair, encodePair, pairLabelOf } from '../../store/modelSelection';
import { displayModelName } from '../../store/thread-events/exchange';
import { openIntelligenceLadder } from '../../store/actions/menu';
import { errorDetail } from '../../utils/errorDetail';
import { providerStepOptions, providerStepCommit } from '../shared/ModelSelectionPicker';
import type { RungProviderStep } from './RungPicker';
import { rungIntelligence, stepTargets, type ShownRung } from './rungControlParts';
import { LADDER_LINK_VALUE, rungListRows } from './rungRows';
import { askRungSwitch } from './rungSwitch';

function withoutContextTwin(model: string): string {
  return model.replace(/\[1m\]$/, '');
}

export interface ChatRungStateProps {
  threadId?: string;
  composeContext: boolean;
}

export function useChatRungState({ threadId, composeContext: perDraft }: ChatRungStateProps) {
  if (chatModels.value.status === 'not-loaded') void loadChatModels();
  if (chatLadder.value.status === 'not-loaded') void loadChatLadder();
  useEffect(() => {
    if (chatLadder.peek().status === 'failed') void loadChatLadder();
  }, []);

  const thread = threadId ? threadMap.value.get(threadId) : undefined;
  const running = !perDraft && !!thread && ['running', 'waiting_for_user_answer', 'paused'].includes(effectiveThreadStatus(thread));
  const turn = chatTurnSelection(threadId);

  const modelValue = perDraft ? resolveModel(threadId) : resolveActiveThreadModel(threadId);
  const effortValue = perDraft
    ? resolveReasoningEffort(threadId)
    : resolveActiveThreadReasoningEffort(threadId, modelValue);
  const effortFor = (model: string) => (perDraft
    ? accountChatEffort(model)
    : resolveActiveThreadReasoningEffort(threadId, model));
  const providerFor = (model: string) => (perDraft
    ? (model === modelValue ? resolveProvider(threadId) : null)
    : resolveActiveThreadProvider(threadId, model));

  function applyDraftPick(patch: ModelSelectionPatch) {
    updateComposeSelection(threadId ?? null, {
      model: patch.model,
      reasoningEffort: patch.reasoningEffort ?? undefined,
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

  const ladder = chatLadder.value;
  const served = ladder.status === 'loaded' ? ladder.data : null;
  const servedRungs = served?.rungs ?? [];
  const defaultRung = served?.default_rung ?? null;
  const routable = new Set(selection.rows.map((r) => withoutContextTwin(r.value)));
  const onRungs = servedRungs.filter((r) => r.on && routable.has(withoutContextTwin(r.model)));
  const labelOf = (model: string) => selection.rows.find((r) => r.value === model)?.label ?? displayModelName(model);
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

  const at = onRungs.findIndex((r) => encodePair(r.model, r.effort) === rungList.current);
  const face: ShownRung = {
    name: labelOf(modelValue),
    effort: selection.effort,
    position: at === -1 ? null : { at: at + 1, of: onRungs.length },
    intelligence: at === -1 ? null : rungIntelligence(onRungs[at]),
  };
  if (running && turn.live?.model) {
    const live = turn.live;
    const liveAt = onRungs.findIndex(r => encodePair(r.model, r.effort) === encodePair(live.model!, live.reasoning_effort || null));
    face.name = labelOf(live.model!);
    face.effort = live.reasoning_effort || null;
    face.position = liveAt < 0 ? null : { at: liveAt + 1, of: onRungs.length };
    face.intelligence = liveAt < 0 ? null : rungIntelligence(onRungs[liveAt]);
  }
  if (running && turn.pending) {
    face.nextTurn = `${labelOf(turn.pending.model ?? modelValue)}${turn.pending.reasoning_effort ? ` · ${turn.pending.reasoning_effort}` : ''}${turn.pending.provider ? ` · ${turn.pending.provider}` : ''}`;
  }
  const shown = [ladder, chatModels.value].every((read) => read.status === 'loaded' || read.status === 'failed')
    ? face
    : null;

  function runModelOf(model: string): string {
    return withoutContextTwin(model) === withoutContextTwin(modelValue) ? modelValue : model;
  }

  function rungPick(value: string, provider?: string): { encoded: string; provider?: string } {
    const pair = decodePair(value);
    const model = runModelOf(pair.model);
    const row = selection.rows.find((r) => r.value === model);
    if (provider && row) return { encoded: providerStepCommit(row, provider, pair.effort), provider };
    return { encoded: encodePair(model, pair.effort) };
  }

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
    if (estimate.rebuilds_cache === 'no') return true;
    const choice = await askRungSwitch({
      from: selection.label,
      to: label,
      extraCostUsd: estimate.extra_cost_usd,
      rebuild: estimate.rebuilds_cache,
      canHandOff: false,
    });
    return choice === 'switch';
  }

  async function recordStartedPick(startedThreadId: string, encoded: string, provider: string | undefined) {
    const label = pairLabelOf(selection.rows, encoded, provider);
    if (!await startedPickProceeds(startedThreadId, encoded, provider, label)) return false;
    const pair = decodePair(encoded);
    try {
      await recordChatSettings(startedThreadId, {
        model: pair.model,
        reasoningEffort: pair.effort ?? undefined,
        provider,
      });
    } catch (err) {
      showToast(`Could not switch to ${label}: ${errorDetail(err)}`, 'error');
      return false;
    }
    if (provider) void rememberProvider(pair.model, provider);
    return true;
  }

  async function pick(encoded: string, provider?: string, quiet = false) {
    const label = pairLabelOf(selection.rows, encoded, provider);
    if (perDraft || !threadId) selection.pick(encoded, provider);
    else if (!await recordStartedPick(threadId, encoded, provider)) return;
    if (!quiet) showToast(`${running ? 'Next turn' : 'Model'}: ${label}`, 'success');
  }

  function pickRung(value: string, provider?: string) {
    if (value === LADDER_LINK_VALUE) {
      openIntelligenceLadder('chat');
      return;
    }
    const commit = rungPick(value, provider);
    void pick(commit.encoded, commit.provider);
  }

  const steps = stepTargets(rungList.rows, rungList.current);

  function step(direction: -1 | 1) {
    const target = direction === -1 ? steps.prev : steps.next;
    if (target === null) return;
    const commit = rungPick(target);
    void pick(commit.encoded, undefined, true);
  }

  return { lockedReason: null, shown, rungList, providerStep, pickRung, steps, step };
}
