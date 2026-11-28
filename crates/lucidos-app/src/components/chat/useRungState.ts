import { rungLockedReason } from './rungLockedReason';
import { useSignal, useSignalEffect, untracked } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import {
  showToast, codingAgentSessionVersion, scopeToRepoId, engineRestarting, threadMap,
  effectiveThreadStatus, successorCardThreadId, successorCardPick,
} from '../../store/store';
import { resolveScope, getComposeSelectionOverride } from '../../store/composeSelections';
import { updateComposeSelection } from '../../store/actions/compose';
import { sendCodingAgentControl } from '../../store/actions/chat-claude-code';
import {
  fetchCodingAgentCommands, fetchSwitchEstimate, type CodingAgentCommandDef, type CodingAgentCommandOption,
  type CodingAgentCommandsResponse, type CodingAgentLadderRung, type CodingAgentModelValue,
  type CodingAgentReasoningEffort, type SuccessorImplementer,
} from '../../api/client';
import type { CodingAgent } from '../../api/types';
import { errorDetail } from '../../utils/errorDetail';
import { failedIfFresh, loadedOr, setLoadingIfFresh, type Loadable } from '../../store/types';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';
import { decodePair, type ModelChoice, type TierChoice } from '../../store/modelSelection';
import { openIntelligenceLadder } from '../../store/actions/menu';
import { displayModelName } from '../../store/thread-events/exchange';
import { LADDER_LINK_VALUE, rungListRows } from './rungRows';
import { shownRung, stepTargets, type ShownRung } from './rungControlParts';
import { menuCodingAgent } from './ControlMenuTrigger';
import { CODING_AGENT_NAME } from './agentExitCopy';
import { askRungSwitch, registerSendGate } from './rungSwitch';
import { continueInSuccessor } from '../../store/actions/successor';

const MAX_RETRIES = 10;
const RETRY_DELAY_MS = 1000;

export function pickerRowValue(options: readonly CodingAgentCommandOption[], model: string): string {
  if (options.some(o => o.value === model)) return model;
  return options.find(o => o.value !== 'default' && o.resolved_model === model)?.value ?? model;
}

interface RungData {
  commands: CodingAgentCommandDef[];
  currentModel: CodingAgentModelValue | null;
  currentEffort: CodingAgentReasoningEffort | null;
  defaultImplementer: CodingAgentCommandsResponse['default_implementer'];
  ladderRungs: CodingAgentLadderRung[];
}

const lastRead = new Map<string, RungData>();

function cachedRead(requestKey: string): Loadable<RungData> {
  const data = lastRead.get(requestKey);
  return data ? { status: 'loaded', data } : { status: 'not-loaded' };
}

const NO_DATA: RungData = {
  commands: [],
  currentModel: null,
  currentEffort: null,
  defaultImplementer: null,
  ladderRungs: [],
};

export interface RungStateProps {
  threadId?: string;
  composeThreadId?: string;
  codingAgent?: CodingAgent | null;
}

export function useRungState({ threadId, composeThreadId, codingAgent }: RungStateProps) {
  const loadSeqRef = useRef(0);
  const retryTimerRef = useRef<number | null>(null);
  const retryCountRef = useRef(0);
  const inCompose = !threadId;
  const agent = menuCodingAgent(codingAgent, threadId, composeThreadId);
  const requestKey = threadId ?? `compose:${scopeToRepoId(resolveScope(composeThreadId)) ?? ''}:${agent}`;
  const read = useSignal<Loadable<RungData>>(cachedRead(requestKey));
  const sending = useSignal(false);
  const { commands, currentModel, currentEffort, defaultImplementer, ladderRungs } =
    loadedOr(read.value, NO_DATA);

  const draftOverride = inCompose ? getComposeSelectionOverride(composeThreadId) : null;
  const heldCardPick = () => threadId && successorCardThreadId.value === threadId
    && successorCardPick.value?.threadId === threadId ? successorCardPick.value : null;
  const cardPick = heldCardPick();
  const effectiveModel = (draftOverride?.ccModel ?? cardPick?.model ?? currentModel) as CodingAgentModelValue | null;
  const effectiveEffort = (draftOverride?.ccReasoningEffort ?? (cardPick ? cardPick.effort : currentEffort)) as CodingAgentReasoningEffort | null;
  const thread = threadId ? threadMap.value.get(threadId) : undefined;
  const lockedReason = rungLockedReason(thread ? effectiveThreadStatus(thread) : null);

  const optionsOf = (subtype: string) =>
    commands.find(c => c.subtype === subtype)?.params[0]?.options ?? [];
  const modelOptions = optionsOf('set_model');
  const tierVocabulary: TierChoice[] = optionsOf('set_reasoning_effort');
  const modelChoices: ModelChoice[] = modelOptions.map(o => ({
    value: o.value,
    label: o.label,
    description: o.description,
    reasoningEfforts: o.reasoning_efforts ?? [],
  }));
  const selection = useModelSelection({
    models: modelChoices,
    vocabulary: tierVocabulary,
    model: effectiveModel ? pickerRowValue(modelOptions, effectiveModel) : 'default',
    effort: effectiveEffort,
    labelFor: displayModelName,
    onChange: applySelectionOverride,
  });

  function applySelectionOverride(patch: ModelSelectionPatch) {
    if (!inCompose) return;
    const model = patch.model === 'default' ? null : patch.model as CodingAgentModelValue;
    const effort = patch.reasoningEffort as CodingAgentReasoningEffort | null;
    updateComposeSelection(composeThreadId ?? null, { ccModel: model, ccReasoningEffort: effort });
  }

  function clearRetryTimer() {
    if (retryTimerRef.current !== null) {
      clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
  }

  function setLoaded(key: string, data: RungData) {
    lastRead.set(key, data);
    read.value = { status: 'loaded', data };
  }

  function load() {
    untracked(loadNow);
  }

  function loadNow() {
    clearRetryTimer();
    if (engineRestarting.value) return;
    const repoId = threadId ? undefined : (scopeToRepoId(resolveScope(composeThreadId)) ?? '');
    const requestAgent = threadId ? undefined : agent;
    const seq = ++loadSeqRef.current;
    setLoadingIfFresh(read);
    fetchCodingAgentCommands(threadId, repoId, requestAgent)
      .then((res) => {
        if (seq !== loadSeqRef.current) return;
        setLoaded(requestKey, {
          commands: res.control_commands,
          currentModel: (res.current_model as CodingAgentModelValue) ?? null,
          currentEffort: (res.current_reasoning_effort as CodingAgentReasoningEffort) ?? null,
          defaultImplementer: res.default_implementer ?? null,
          ladderRungs: res.ladder_rungs ?? [],
        });
        retryCountRef.current = 0;
      })
      .catch((err: unknown) => {
        if (seq !== loadSeqRef.current) return;
        if (retryCountRef.current < MAX_RETRIES) {
          retryCountRef.current++;
          retryTimerRef.current = window.setTimeout(load, RETRY_DELAY_MS);
        } else {
          read.value = failedIfFresh(read.value, err);
          showToast(`Failed to load the model list: ${errorDetail(err)}`, 'error');
        }
      });
  }

  useEffect(() => {
    retryCountRef.current = 0;
    read.value = cachedRead(requestKey);
    load();
    return clearRetryTimer;
  }, [requestKey]);

  useSignalEffect(() => {
    if (codingAgentSessionVersion.value > 0) {
      retryCountRef.current = 0;
      load();
    }
  });

  const wasRestartingRef = useRef(false);
  useSignalEffect(() => {
    const restarting = engineRestarting.value;
    const transitionedToReady = wasRestartingRef.current && !restarting;
    wasRestartingRef.current = restarting;
    if (transitionedToReady) {
      retryCountRef.current = 0;
      load();
    }
  });

  const defaultPair = defaultImplementer
    ? { model: defaultImplementer.model, effort: effectiveEffort ?? defaultImplementer.effort }
    : null;

  const rungList = rungListRows({
    rungs: ladderRungs,
    options: modelOptions,
    model: effectiveModel,
    effort: selection.effort,
    modelLabel: displayModelName,
    defaultPair,
  });

  const shown: ShownRung | null = shownRung({
    loaded: read.value.status === 'loaded',
    rungs: ladderRungs,
    options: modelOptions,
    model: effectiveModel,
    effort: effectiveEffort,
    defaultPair,
    agentName: CODING_AGENT_NAME[agent],
    modelLabel: displayModelName,
  });

  function patchData(patch: Partial<RungData>) {
    const loadedData = read.value;
    if (loadedData.status === 'loaded') setLoaded(requestKey, { ...loadedData.data, ...patch });
  }

  function runModelOf(model: string): string {
    return modelOptions.find(o => o.value === model)?.resolved_model ?? model;
  }

  async function switchProceeds(
    startedThreadId: string,
    pair: { model: string; effort: string | null },
    label: string,
    alwaysAsk = false,
  ): Promise<boolean> {
    let estimate;
    try {
      estimate = await fetchSwitchEstimate(startedThreadId, runModelOf(pair.model), pair.effort);
    } catch (err) {
      showToast(`Could not check what switching to ${label} costs: ${errorDetail(err)}`, 'error');
      return false;
    }
    if (estimate.rebuilds_cache === 'no' && !alwaysAsk) return true;
    const implementer: SuccessorImplementer = { coding_agent_model: pair.model, reasoning_effort: pair.effort };
    const startedThread = threadMap.value.get(startedThreadId);
    const choice = await askRungSwitch({
      from: rungList.rows.find(r => r.value === rungList.current)?.label ?? 'this rung',
      to: label,
      extraCostUsd: estimate.extra_cost_usd,
      rebuild: estimate.rebuilds_cache,
      canHandOff: startedThread !== undefined && effectiveThreadStatus(startedThread) === 'idle',
    });
    if (choice === 'handoff') {
      try {
        await continueInSuccessor(startedThreadId, implementer);
      } catch (err) {
        showToast(`Could not continue in a fresh session on ${label}: ${errorDetail(err)}`, 'error');
      }
    }
    return choice === 'switch';
  }

  async function recordPick(startedThreadId: string, pair: { model: string; effort: string | null }): Promise<boolean> {
    const cmd = commands.find(c => c.subtype === 'set_model');
    if (!cmd) return false;
    const effort = pair.effort as CodingAgentReasoningEffort | null;
    const request = { subtype: cmd.subtype, [cmd.params[0].key]: pair.model, ...(effort ? { effort } : {}) };
    if (!await sendCodingAgentControl(startedThreadId, request)) return false;
    patchData({ currentModel: pair.model as CodingAgentModelValue, ...(effort ? { currentEffort: effort } : {}) });
    return true;
  }

  function cardPickRidesSend(sendThreadId: string): boolean {
    return sendThreadId === threadId && heldCardPick() !== null;
  }

  async function cardPickSendProceeds(sendThreadId: string): Promise<boolean> {
    const held = heldCardPick();
    if (!held) return true;
    const pair = { model: held.model, effort: held.effort };
    const label = [displayModelName(pair.model), pair.effort].filter(Boolean).join(' · ');
    if (!await switchProceeds(sendThreadId, pair, label, true)) return false;
    if (!await recordPick(sendThreadId, pair)) return false;
    successorCardPick.value = null;
    return true;
  }
  const sendGateRef = useRef({ asks: cardPickRidesSend, proceeds: cardPickSendProceeds });
  sendGateRef.current = { asks: cardPickRidesSend, proceeds: cardPickSendProceeds };
  useEffect(() => registerSendGate({
    asks: (id) => sendGateRef.current.asks(id),
    proceeds: (id) => sendGateRef.current.proceeds(id),
  }), []);

  async function pickModelSelection(encoded: string, label: string, quiet = false) {
    const cmd = commands.find(c => c.subtype === 'set_model');
    if (!cmd) return;
    const pair = decodePair(encoded);
    if (!threadId) {
      selection.pick(encoded);
      if (!quiet) showToast(`${cmd.label}: ${label}`, 'success');
      return;
    }
    if (successorCardThreadId.value === threadId) {
      successorCardPick.value = { threadId, model: pair.model, effort: pair.effort };
      return;
    }
    if (lockedReason) {
      showToast(lockedReason, 'info');
      return;
    }
    sending.value = true;
    try {
      if (!await switchProceeds(threadId, pair, label)) return;
      if (!await recordPick(threadId, pair)) return;
      if (!quiet) showToast(`${cmd.label}: ${label}`, 'success');
    } finally {
      sending.value = false;
    }
  }

  function pickRung(value: string) {
    if (value === LADDER_LINK_VALUE) {
      openIntelligenceLadder(agent);
      return;
    }
    const label = rungList.rows.find(r => r.value === value)?.label ?? value;
    void pickModelSelection(value, label);
  }

  const steps = stepTargets(rungList.rows, rungList.current);

  function step(direction: -1 | 1) {
    const target = direction === -1 ? steps.prev : steps.next;
    if (target === null || sending.value) return;
    if (lockedReason) {
      showToast(lockedReason, 'info');
      return;
    }
    const label = rungList.rows.find(r => r.value === target)?.label ?? target;
    void pickModelSelection(target, label, true);
  }

  return { lockedReason, sending, shown, rungList, pickRung, steps, step };
}
