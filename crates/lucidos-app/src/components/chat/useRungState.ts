import { useSignal, useSignalEffect, untracked } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import {
  showToast, settingsScrollTarget, codingAgentSessionVersion, codingAgentPendingModel,
  codingAgentPendingReasoningEffort, scopeToRepoId, engineRestarting, threadMap, effectiveThreadStatus,
  successorCardThreadId,
} from '../../store/store';
import { resolveScope, getComposeSelectionOverride } from '../../store/composeSelections';
import { updateComposeSelection } from '../../store/actions/compose';
import { sendCodingAgentControl, sessionRecorded, type ControlSendResult } from '../../store/actions/chat-claude-code';
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
import { openSettingsSubview } from '../../store/actions/menu';
import { ladderAnchor } from '../settings/intelligenceLadder';
import { displayModelName } from '../../store/thread-events/exchange';
import { controlPickToast } from './controlPickToast';
import { LADDER_LINK_VALUE, rungListRows } from './rungRows';
import { rungFaces, shownRung, stepTargets, type ShownRung } from './rungControlParts';
import { menuCodingAgent } from './ControlMenuTrigger';
import { CODING_AGENT_NAME } from './agentExitCopy';
import { askRungSwitch, registerSendGate } from './rungSwitch';
import { continueInSuccessor } from '../../store/actions/successor';

const MAX_RETRIES = 10;
const RETRY_DELAY_MS = 1000;

/** The row a thread's model shows as: the row with that value, else the
 *  first row other than Default that runs it. A thread records the id Claude
 *  Code reports, which is often no row's own value. */
export function pickerRowValue(options: readonly CodingAgentCommandOption[], model: string): string {
  if (options.some(o => o.value === model)) return model;
  return options.find(o => o.value !== 'default' && o.resolved_model === model)?.value ?? model;
}

/** What one commands read says about the thread's rungs. */
interface RungData {
  commands: CodingAgentCommandDef[];
  currentModel: CodingAgentModelValue | null;
  currentEffort: CodingAgentReasoningEffort | null;
  defaultImplementer: CodingAgentCommandsResponse['default_implementer'];
  ladderRungs: CodingAgentLadderRung[];
  hasActiveSession: boolean;
}

const NO_DATA: RungData = {
  commands: [],
  currentModel: null,
  currentEffort: null,
  defaultImplementer: null,
  ladderRungs: [],
  hasActiveSession: false,
};

export interface RungStateProps {
  threadId?: string;
  /** The focused COMPOSING draft's id, set only while composing (mutually
   *  exclusive with `threadId`, the active-session id). Compose picks live in
   *  this draft's own override, so a pick on one draft cannot leak to another. */
  composeThreadId?: string;
  codingAgent?: CodingAgent | null;
}

/** What the rung control knows about the coding-agent thread it belongs to:
 *  the ladder's rungs, the pair in force, and how a pick commits.
 *
 *  Compose (`!threadId`) writes the draft's override, or the pending slot
 *  before a draft exists. An active thread writes the global pending signals,
 *  which a read clears once the live session has adopted the value. */
export function useRungState({ threadId, composeThreadId, codingAgent }: RungStateProps) {
  const loadSeqRef = useRef(0);
  const retryTimerRef = useRef<number | null>(null);
  const retryCountRef = useRef(0);
  const read = useSignal<Loadable<RungData>>({ status: 'not-loaded' });
  const sending = useSignal(false);
  const { commands, currentModel, currentEffort, defaultImplementer, ladderRungs, hasActiveSession } =
    loadedOr(read.value, NO_DATA);

  const inCompose = !threadId;
  const agent = menuCodingAgent(codingAgent, threadId, composeThreadId);
  const draftOverride = inCompose ? getComposeSelectionOverride(composeThreadId) : null;
  const pendingModel: CodingAgentModelValue | null = draftOverride
    ? (draftOverride.ccModel ?? null)
    : codingAgentPendingModel.value;
  const pendingEffort: CodingAgentReasoningEffort | null = draftOverride
    ? (draftOverride.ccReasoningEffort ?? null)
    : codingAgentPendingReasoningEffort.value;
  const effectiveModel = pendingModel ?? currentModel;
  const effectiveEffort = pendingEffort ?? currentEffort;

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
    // No model picked yet means the backend's own default, which IS the
    // `default` row. Asking that row keeps the compose view (no session, no
    // recorded model) offering the universally-accepted tiers.
    model: effectiveModel ? pickerRowValue(modelOptions, effectiveModel) : 'default',
    effort: effectiveEffort,
    // Claude Code's picker is what Claude Code lists now, so a thread can be
    // pinned to a model it has dropped. Name it rather than show the bare id.
    labelFor: displayModelName,
    onChange: applySelectionOverride,
  });

  /** Record a pick where this surface's picks live, never an account
   *  preference. Both halves land together, because the pair is picked whole. */
  function applySelectionOverride(patch: ModelSelectionPatch) {
    const model = patch.model === 'default' ? null : patch.model as CodingAgentModelValue;
    const effort = patch.reasoningEffort as CodingAgentReasoningEffort | null;
    if (inCompose) {
      updateComposeSelection(composeThreadId ?? null, { ccModel: model, ccReasoningEffort: effort });
      return;
    }
    codingAgentPendingModel.value = model;
    codingAgentPendingReasoningEffort.value = effort;
  }

  function clearRetryTimer() {
    if (retryTimerRef.current !== null) {
      clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
  }

  /** An action, never a dependency: its response writes signals this hook
   *  reads, so none of them may subscribe the effect that called it. */
  function load() {
    untracked(loadNow);
  }

  function loadNow() {
    clearRetryTimer();
    // A restart outlasts the retry budget. The engineRestarting effect below
    // reloads once the engine is back.
    if (engineRestarting.value) return;
    // The compose view scopes the request to the draft's repo, as the `/` menu
    // does, so the two share one read.
    const repoId = threadId ? undefined : (scopeToRepoId(resolveScope(composeThreadId)) ?? '');
    const requestAgent = threadId ? undefined : agent;
    const seq = ++loadSeqRef.current;
    setLoadingIfFresh(read);
    fetchCodingAgentCommands(threadId, repoId, requestAgent)
      .then((res) => {
        if (seq !== loadSeqRef.current) return;
        // Clear a pending pick ONLY when the live session has adopted that exact
        // value. A bare `has_active_session` check would clear a pick made
        // while an older read was in flight, and the next send would lose it.
        if (res.has_active_session) {
          if (res.current_reasoning_effort === codingAgentPendingReasoningEffort.value) {
            codingAgentPendingReasoningEffort.value = null;
          }
          if (res.current_model === codingAgentPendingModel.value) {
            codingAgentPendingModel.value = null;
          }
        }
        read.value = {
          status: 'loaded',
          data: {
            commands: res.control_commands,
            currentModel: (res.current_model as CodingAgentModelValue) ?? null,
            currentEffort: (res.current_reasoning_effort as CodingAgentReasoningEffort) ?? null,
            defaultImplementer: res.default_implementer ?? null,
            ladderRungs: res.ladder_rungs ?? [],
            hasActiveSession: res.has_active_session,
          },
        };
        retryCountRef.current = 0;
      })
      .catch((err: unknown) => {
        if (seq !== loadSeqRef.current) return;
        // A stale iOS PWA connection rejects the first read after a wake.
        if (retryCountRef.current < MAX_RETRIES) {
          retryCountRef.current++;
          retryTimerRef.current = window.setTimeout(load, RETRY_DELAY_MS);
        } else {
          read.value = failedIfFresh(read.value, err);
          showToast(`Failed to load the model list: ${errorDetail(err)}`, 'error');
        }
      });
  }

  // Keyed on what the request sends, read in render, so a prop change counts
  // too. Never a signal effect: it re-runs on signals only, and the control
  // stays mounted across a draft switch.
  const requestKey = threadId ?? `compose:${scopeToRepoId(resolveScope(composeThreadId)) ?? ''}:${agent}`;
  useEffect(() => {
    retryCountRef.current = 0;
    read.value = { status: 'not-loaded' };
    load();
    return clearRetryTimer;
  }, [requestKey]);

  // A session starting or resuming changes the pair in force.
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

  // A thread with no model of its own runs the default implementer's model, at
  // its own effort if it has one. The engine's `fill_pick` does the same.
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
  const faces = rungFaces(ladderRungs, (rung) => [rung.label]);

  /** Record what a control request just changed on the live session. */
  function patchData(patch: Partial<RungData>) {
    const loadedData = read.value;
    if (loadedData.status === 'loaded') read.value = { status: 'loaded', data: { ...loadedData.data, ...patch } };
  }

  /** The model a pick runs, as the engine's price table names it. */
  function runModelOf(model: string): string {
    return modelOptions.find(o => o.value === model)?.resolved_model ?? model;
  }

  /** Whether a pick on a started thread goes ahead, whether or not its session
   *  is live. A switch that keeps the cache is silent. One that rebuilds it
   *  asks first, and only `switch` lets the caller store the pick. A handoff
   *  starts the new thread here. */
  async function switchProceeds(
    startedThreadId: string,
    pair: { model: string; effort: string | null },
    label: string,
  ): Promise<boolean> {
    let estimate;
    try {
      estimate = await fetchSwitchEstimate(startedThreadId, runModelOf(pair.model), pair.effort);
    } catch (err) {
      showToast(`Could not check what switching to ${label} costs: ${errorDetail(err)}`, 'error');
      return false;
    }
    if (!estimate.rebuilds_cache) return true;
    const implementer: SuccessorImplementer = { coding_agent_model: pair.model, reasoning_effort: pair.effort };
    const thread = threadMap.value.get(startedThreadId);
    const choice = await askRungSwitch({
      from: rungList.rows.find(r => r.value === rungList.current)?.label ?? 'this rung',
      to: label,
      extraCostUsd: estimate.extra_cost_usd,
      canHandOff: thread !== undefined && effectiveThreadStatus(thread) === 'idle',
    });
    if (choice === 'handoff') {
      try {
        await continueInSuccessor(startedThreadId, implementer);
      } catch (err) {
        showToast(`Could not continue in a new thread on ${label}: ${errorDetail(err)}`, 'error');
      }
    }
    return choice === 'switch';
  }

  /** Whether a send in this thread owes the rung switch question: the
   *  *Successor card* shows and holds a pick. A pick under the card aims the
   *  successor and switches nothing, yet it sits in the pending pair that any
   *  send in this thread carries. */
  function cardPickRidesSend(sendThreadId: string): boolean {
    return sendThreadId === threadId && successorCardThreadId.value === threadId
      && (pendingModel !== null || pendingEffort !== null) && !!effectiveModel;
  }

  function cardPickSendProceeds(sendThreadId: string): Promise<boolean> {
    const model = effectiveModel;
    if (!model) return Promise.resolve(true);
    const label = [displayModelName(model), effectiveEffort].filter(Boolean).join(' · ');
    return switchProceeds(sendThreadId, { model, effort: effectiveEffort }, label);
  }
  const sendGateRef = useRef({ asks: cardPickRidesSend, proceeds: cardPickSendProceeds });
  sendGateRef.current = { asks: cardPickRidesSend, proceeds: cardPickSendProceeds };
  useEffect(() => registerSendGate({
    asks: (id) => sendGateRef.current.asks(id),
    proceeds: (id) => sendGateRef.current.proceeds(id),
  }), []);

  /** Apply a whole *model selection*, the picker's final answer.
   *
   *  A started thread asks first when the pick rebuilds its cache. With no live
   *  session the pick is stored for the next spawn and nothing is sent.
   *  A live session is reconciled with TWO control requests, sent in order, so
   *  the model is recorded before the tier it accepts. A Claude Code tier
   *  restarts the session, so it applies at once. If the session exits
   *  between them, the pending pick recorded above carries the tier into the
   *  next spawn or follow-up instead. */
  async function pickModelSelection(encoded: string, label: string, quiet = false) {
    const cmd = commands.find(c => c.subtype === 'set_model');
    if (!cmd) return;
    const pair = decodePair(encoded);
    if (!threadId) {
      selection.pick(encoded);
      if (!quiet) showToast(`${cmd.label}: ${label}`, 'success');
      return;
    }
    // Under the Successor card a pick aims the successor, and this thread keeps
    // its model. The card names the pick.
    if (successorCardThreadId.value === threadId) {
      selection.pick(encoded);
      return;
    }
    sending.value = true;
    if (!await switchProceeds(threadId, pair, label)) {
      sending.value = false;
      return;
    }
    selection.pick(encoded);
    if (!hasActiveSession) {
      sending.value = false;
      if (!quiet) showToast(`${cmd.label}: ${label}`, 'success');
      return;
    }
    const result = await sendCodingAgentControl(threadId, {
      subtype: cmd.subtype,
      [cmd.params[0].key]: pair.model,
    });
    let tierResult: ControlSendResult | null = null;
    if (sessionRecorded(result)) patchData({ currentModel: pair.model as CodingAgentModelValue });
    if (sessionRecorded(result) && pair.effort) {
      const tier = pair.effort as CodingAgentReasoningEffort;
      tierResult = await sendCodingAgentControl(threadId, {
        subtype: 'set_reasoning_effort',
        effort: tier,
      });
      if (sessionRecorded(tierResult)) patchData({ currentEffort: tier });
    }
    sending.value = false;
    if (result !== 'error' && tierResult !== 'error' && !quiet) {
      showToast(controlPickToast(cmd.label, label, result, tierResult), 'success');
    }
  }

  /** Commit a rung, or follow the ladder link out to the editor. */
  function pickRung(value: string) {
    if (value === LADDER_LINK_VALUE) {
      openSettingsSubview('intelligence');
      settingsScrollTarget.value = ladderAnchor(agent);
      return;
    }
    const label = rungList.rows.find(r => r.value === value)?.label ?? value;
    void pickModelSelection(value, label);
  }

  const steps = stepTargets(rungList.rows, rungList.current);

  /** Move one row up (-1) or down (1) the rung list. The label shows the new
   *  pair, so a step says nothing more. At either end it does nothing. */
  function step(direction: -1 | 1) {
    const target = direction === -1 ? steps.prev : steps.next;
    if (target === null || sending.value) return;
    const label = rungList.rows.find(r => r.value === target)?.label ?? target;
    void pickModelSelection(target, label, true);
  }

  return { sending, shown, faces, rungList, pickRung, steps, step };
}
