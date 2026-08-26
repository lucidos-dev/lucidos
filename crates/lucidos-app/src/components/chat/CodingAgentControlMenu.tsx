import { Fragment } from 'preact';
import { useSignal, useSignalEffect, signal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { showToast, codingAgentSessionVersion, codingAgentPendingModel, codingAgentPendingReasoningEffort, scopeToRepoId, engineRestarting } from '../../store/store';
import { resolveScope, resolveCodingAgent, getComposeSelectionOverride } from '../../store/composeSelections';
import { updateComposeSelection } from '../../store/actions/compose';
import { sendCodingAgentControl } from '../../store/actions/chat-claude-code';
import { sendMessage } from '../../store/actions/chat';
import { fetchCodingAgentCommands, type CodingAgentCommandDef, type CodingAgentCommandsResponse, type CodingAgentModelValue, type CodingAgentReasoningEffort } from '../../api/client';
import type { CodingAgent } from '../../api/types';
import { ClaudeIcon, CodexIcon } from '../shared/icons';
import { focusIfNeeded, isTextInput } from '../../utils/dom';
import { errorDetail } from '../../utils/errorDetail';
import { Overlay } from '../shared/Overlay';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';
import {
  decodePair, pairLabelOf, type ModelChoice, type ModelRow, type TierChoice,
} from '../../store/modelSelection';
import { ControlOptionList, type ControlOption } from '../shared/ControlOptionList';
import { ModelSelectionPicker } from '../shared/ModelSelectionPicker';
import { FrontendPreviewSection } from './FrontendPreviewSection';
import { loadFrontendPreview } from '../../store/actions/frontend-preview';

// Signal for PromptInput to request opening the menu with a filter
// Set to a string (the filter text) to open, consumed by the component
export const codingAgentMenuOpenRequest = signal<string | null>(null);

// Module-level signals — survive unmount/remount so button appears instantly.
// Model/effort are NOT cached here — they're per-thread (from backend events).
const persistedControlCommands = signal<CodingAgentCommandDef[] | null>(null);
const persistedBuiltinCommands = signal<string[] | null>(null);
const persistedSkillCommands = signal<string[] | null>(null);
const persistedHasActiveSession = signal(false);

// Max retries when commands come back empty (CC Init handshake may be in-flight)
const MAX_EMPTY_RETRIES = 10;
const RETRY_DELAY_MS = 1000;

/** True when CC has reported builtin or skill slash commands (CC binary connected).
 *  Only gates slash command cache updates — model/effort are updated independently. */
function codingAgentSlashCommandsReady(builtin: string[], skill: string[]): boolean {
  return builtin.length > 0 || skill.length > 0;
}

/** True when any commands are available (control, builtin, or skill). */
function hasAnyCommands(control: unknown[], builtin: string[], skill: string[]): boolean {
  return control.length > 0 || codingAgentSlashCommandsReady(builtin, skill);
}

type ListItem =
  | { type: 'control'; subtype: string; label: string }
  | { type: 'slash'; name: string };

/** The control commands to OFFER.
 *
 *  A *model selection* is one thing, so it is one entry: the `set_model` rows
 *  carry the tier too. The backend still serves `set_reasoning_effort`, and the
 *  request this menu sends to reconcile a live session still uses it. It is
 *  never a row the user picks on its own. */
export function offeredControlCommands(
  commands: readonly CodingAgentCommandDef[],
): CodingAgentCommandDef[] {
  return commands.filter(c => c.subtype !== 'set_reasoning_effort');
}

interface Props {
  threadId?: string;
  /** The focused COMPOSING draft's id, set only while composing (mutually
   *  exclusive with `threadId`, which is the active-session id). When present the
   *  model/effort picks + skill scope come from THIS draft's per-draft override
   *  (`composeSelections`) instead of the global pending signals / picker, so a
   *  pick on one draft can't leak to another. */
  composeThreadId?: string;
  codingAgent?: CodingAgent | null;
}

export function CodingAgentControlMenu({ threadId, composeThreadId, codingAgent }: Props) {
  const menuRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const optionsListRef = useRef<HTMLDivElement>(null);
  const retryTimerRef = useRef<number | null>(null);
  const retryCountRef = useRef(0);
  const open = useSignal(false);
  const activeCommand = useSignal<string | null>(null);
  const paramValues = useSignal<Record<string, string>>({});
  const sending = useSignal(false);
  const controlCommands = useSignal<CodingAgentCommandDef[]>(persistedControlCommands.peek() ?? []);
  const builtinCommands = useSignal<string[]>(persistedBuiltinCommands.peek() ?? []);
  const skillCommands = useSignal<string[]>(persistedSkillCommands.peek() ?? []);
  const currentModel = useSignal<CodingAgentModelValue | null>(null);
  const currentReasoningEffort = useSignal<CodingAgentReasoningEffort | null>(null);
  const hasActiveSession = useSignal(persistedHasActiveSession.peek());
  const filter = useSignal('');
  const highlightIndex = useSignal(-1);
  // Active threads pass `threadId`; the compose view (a focused draft OR the
  // fresh no-draft view) does not — so `!threadId` IS "compose context". In
  // compose, model/effort picks live per-draft (`composeThreadId`) or, before a
  // draft exists, in the PENDING slot (`composeThreadId` undefined → resolvers
  // route to pending). On an active thread they live in the global pending
  // signals (reconciled per-thread by loadCommands).
  const inCompose = !threadId;
  const resolvedCodingAgent = codingAgent ?? (threadId ? 'claude-code' : resolveCodingAgent(composeThreadId));
  const isClaudeCode = resolvedCodingAgent === 'claude-code';
  // null here = "no pick" → display falls through to the backend's current value.
  const draftOverride = inCompose ? getComposeSelectionOverride(composeThreadId) : null;
  const pendingModel: CodingAgentModelValue | null = draftOverride
    ? (draftOverride.ccModel ?? null)
    : codingAgentPendingModel.value;
  const pendingReasoningEffort: CodingAgentReasoningEffort | null = draftOverride
    ? (draftOverride.ccReasoningEffort ?? null)
    : codingAgentPendingReasoningEffort.value;
  const menuLabel = isClaudeCode ? 'Claude Code' : 'Codex';
  const effectiveBuiltinCommands = isClaudeCode ? builtinCommands.value : [];
  const effectiveSkillCommands = isClaudeCode ? skillCommands.value : [];
  const effectiveModel = pendingModel ?? currentModel.value;
  const selectedReasoningEffort = pendingReasoningEffort ?? currentReasoningEffort.value;
  const optionsOf = (subtype: string) =>
    controlCommands.value.find(command => command.subtype === subtype)?.params[0]?.options ?? [];
  // The served model rows already carry their tiers, so this is the same shape
  // the Lucidos Agent's registry adapter produces.
  const modelChoices: ModelChoice[] = optionsOf('set_model').map(o => ({
    value: o.value,
    label: o.label,
    description: o.description,
    reasoningEfforts: o.reasoning_efforts ?? [],
  }));
  const tierVocabulary: TierChoice[] = optionsOf('set_reasoning_effort');
  // No model picked yet means the backend's own default, which IS the
  // `default` row. Asking that row keeps the compose view (no session, no
  // recorded model) offering the universally-accepted tiers.
  const tierModel = effectiveModel ?? 'default';
  const selection = useModelSelection({
    models: modelChoices,
    vocabulary: tierVocabulary,
    model: tierModel,
    effort: selectedReasoningEffort,
    onChange: applySelectionOverride,
  });

  /** Record a pick where this surface's picks live, never an account
   *  preference. Compose writes the draft's own override, or the pending slot
   *  before a draft exists. An active thread writes the global pending signals,
   *  which `loadCommands` clears once the live session has adopted the value.
   *
   *  Both halves land together, because the pair is picked whole. */
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

  function loadCommands() {
    clearRetryTimer();
    // Restart can take 30-90s (Rust recompile + boot) — longer than the retry
    // budget. Skip and let the engineRestarting useSignalEffect re-trigger us
    // when the engine is back. Same convention as loadAllThreads etc.
    if (engineRestarting.value) return;
    // Compose view (no thread) scopes commands to the user-selected repo so
    // skills from other repos never leak into the menu. For Lucidos and App
    // scopes there's no repo UUID (App skills aren't surfaced this way yet —
    // they'll appear once the live CC session loads its own command list).
    // `scopeToRepoId` returns the external repo UUID or undefined; passing
    // `''` to the backend resolves to the default "Lucidos" repo, which is
    // the right fallback for both Lucidos and App. Compose scope is per-draft.
    const repoId = threadId ? undefined : (scopeToRepoId(resolveScope(composeThreadId)) ?? '');
    // Compose view: the backend picker decides which control menu (CC model
    // list vs Codex model list) the server returns. Thread-bound menus
    // resolve the backend server-side from thread_summaries.
    const requestCodingAgent = threadId ? undefined : resolvedCodingAgent;
    fetchCodingAgentCommands(threadId, repoId, requestCodingAgent)
      .then((res: CodingAgentCommandsResponse) => {
        // Always update control commands (always present from backend)
        persistedControlCommands.value = res.control_commands;
        controlCommands.value = res.control_commands;
        persistedHasActiveSession.value = res.has_active_session;
        hasActiveSession.value = res.has_active_session;
        // Update model/effort from backend response (per-thread, from events).
        // Active session: values come from the live session. No session: values
        // come from CodingAgentSettingsChanged events for this thread (may be null).
        // Clear pending ONLY when the live session has adopted that exact value.
        // A bare `has_active_session` check would clear the user's pending pick
        // from a stale in-flight fetch issued before the click — the next send
        // then loses the override and the spawn uses the prior session's
        // effort. Matching the value first guarantees we only clear pending
        // when this response actually proves it landed.
        if (res.has_active_session) {
          if (res.current_reasoning_effort === codingAgentPendingReasoningEffort.value) {
            codingAgentPendingReasoningEffort.value = null;
          }
          if (res.current_model === codingAgentPendingModel.value) {
            codingAgentPendingModel.value = null;
          }
        }
        currentReasoningEffort.value = (res.current_reasoning_effort as CodingAgentReasoningEffort) ?? null;
        currentModel.value = (res.current_model as CodingAgentModelValue) ?? null;
        if (codingAgentSlashCommandsReady(res.builtin_commands, res.skill_commands)) {
          // Got real commands — update persisted state
          persistedBuiltinCommands.value = res.builtin_commands;
          persistedSkillCommands.value = res.skill_commands;
          builtinCommands.value = res.builtin_commands;
          skillCommands.value = res.skill_commands;
          retryCountRef.current = 0;
        } else {
          // Empty response — keep existing cached commands, retry for fresh ones
          // (only retry when a Claude Code thread exists — compose view has
          // no session to wait for, and Codex has no slash command surface)
          if (isClaudeCode && threadId && retryCountRef.current < MAX_EMPTY_RETRIES) {
            retryCountRef.current++;
            retryTimerRef.current = window.setTimeout(loadCommands, RETRY_DELAY_MS);
          }
        }
      })
      .catch((err: unknown) => {
        // Retry both views: iOS PWA HTTP/2 connections go stale after
        // backgrounding and the first wake fetch rejects with
        // TypeError("Load failed"). The threadId guard in the empty-response
        // branch above is about "no session to wait for" — a different concern.
        if (retryCountRef.current < MAX_EMPTY_RETRIES) {
          retryCountRef.current++;
          retryTimerRef.current = window.setTimeout(loadCommands, RETRY_DELAY_MS);
        } else {
          // The reason, not a bare generic: this fires only after every retry
          // is spent, so it is the one report the user gets.
          showToast(`Failed to load coding-agent commands: ${errorDetail(err)}`, 'error');
        }
      });
  }

  /** Open the command menu with an optional filter (from "/" prefix). */
  function openMenu(filterText = '') {
    if (!hasAnyCommands(controlCommands.value, effectiveBuiltinCommands, effectiveSkillCommands)) return;
    open.value = true;
    filter.value = filterText;
    highlightIndex.value = 0;
    // Reset retry counter — each manual open gets fresh retries
    retryCountRef.current = 0;
    loadCommands();
    // Read the preview slot on open rather than at startup: it is dev-only and
    // one slot per workspace, so the menu opening is both the cheapest and the
    // freshest moment to learn what is running. SSE keeps it live from there.
    if (threadId) void loadFrontendPreview();
  }

  useEffect(() => {
    retryCountRef.current = 0;
    // Compose view (no threadId) is loaded by the selectedScope signal effect
    // below — fires on first render and on every repo switch. Loading here too
    // would double-fetch on mount.
    if (threadId) loadCommands();
    return clearRetryTimer;
  }, [threadId, resolvedCodingAgent]);

  // Blur before unmount so useHideOnScroll's focusout handler fires while
  // the element is still connected, properly resetting keyboardOpen.
  useEffect(() => close, []);

  // Re-fetch commands when a Claude Code session starts/resumes (SSE-driven).
  // useSignalEffect required — see comment below re: Preact signal optimization.
  useSignalEffect(() => {
    if (codingAgentSessionVersion.value > 0) {
      retryCountRef.current = 0;
      loadCommands();
    }
  });

  // Re-fetch compose-view commands when an engine restart completes. The
  // in-flight loadCommands was gated out while engineRestarting was true; this
  // transition is the cue to fire a fresh load. Focused-CC-thread instances
  // skip this — runResumeSync runs loadAllThreads which bumps codingAgentSessionVersion
  // and the effect above already covers them.
  const wasRestartingRef = useRef(false);
  useSignalEffect(() => {
    const restarting = engineRestarting.value;
    const transitionedToReady = wasRestartingRef.current && !restarting;
    wasRestartingRef.current = restarting;
    if (transitionedToReady && !threadId) {
      retryCountRef.current = 0;
      loadCommands();
    }
  });

  // Re-fetch when the compose-view scope dropdown changes — different repo,
  // different skills. Clear the module-level cache first so the previous
  // scope's skills don't flash before the new fetch resolves. Thread-bound
  // menus are bound to the thread's repo and ignore this signal.
  useSignalEffect(() => {
    // Reading the resolved per-draft scope/agent here is what subscribes this
    // effect — it re-fires when this draft's target/backend override changes (or
    // the global default, when there is no draft).
    resolveScope(composeThreadId);
    resolveCodingAgent(composeThreadId);
    if (threadId) return;
    persistedControlCommands.value = null;
    persistedBuiltinCommands.value = null;
    persistedSkillCommands.value = null;
    controlCommands.value = [];
    builtinCommands.value = [];
    skillCommands.value = [];
    retryCountRef.current = 0;
    loadCommands();
  });

  // Open from PromptInput when user types "/" prefix.
  // Must use useSignalEffect (not useEffect) — @preact/signals can optimize
  // away re-renders when a signal doesn't affect DOM output, which prevents
  // useEffect deps from being re-evaluated.
  // Layout check: `App` mounts only the active layout's pane tree (desktop XOR
  // mobile), so exactly one instance of this menu exists. It can still be
  // unlaid-out on the frame the request arrives (zero-size bounding rect), and
  // the menu anchors its overlay to that box, so wait for a real one.
  useSignalEffect(() => {
    const req = codingAgentMenuOpenRequest.value;
    if (req !== null) {
      const el = menuRef.current;
      if (!el || el.getBoundingClientRect().width === 0) {
        // Layout may not have finalized yet — retry after next frame
        if (el) requestAnimationFrame(() => {
          if (codingAgentMenuOpenRequest.value !== null && el.getBoundingClientRect().width > 0) {
            codingAgentMenuOpenRequest.value = null;
            openMenu(req);
          }
        });
        return;
      }
      codingAgentMenuOpenRequest.value = null;
      openMenu(req);
    }
  });

  // Global "/" shortcut to open dropdown when not typing in an input
  useEffect(() => {
    function handleSlash(e: KeyboardEvent) {
      if (open.value) return;
      if (isTextInput(e.target)) return;
      if (e.key === '/') {
        e.preventDefault();
        openMenu();
      }
    }
    document.addEventListener('keydown', handleSlash);
    return () => document.removeEventListener('keydown', handleSlash);
  }, []);

  // Scroll highlighted item into view
  useEffect(() => {
    if (highlightIndex.value < 0) return;
    const el = menuRef.current?.querySelector('.control-item-active');
    el?.scrollIntoView({ block: 'nearest' });
  }, [highlightIndex.value]);

  // Focus filter input when dropdown opens (autoFocus is unreliable for conditional rendering)
  useEffect(() => {
    if (open.value && !activeCommand.value) {
      requestAnimationFrame(() => focusIfNeeded(filterRef.current));
    }
  }, [open.value, activeCommand.value]);

  // Focus the options view on entry, so its arrow keys are live. The model
  // picker focuses whichever of its own two steps is showing.
  useEffect(() => {
    if (activeCommand.value !== null && activeCommand.value !== 'set_model') {
      focusIfNeeded(optionsListRef.current);
    }
  }, [activeCommand.value]);

  function selectCommand(subtype: string) {
    activeCommand.value = subtype;
    paramValues.value = {};
    // The filter box is shared with the command list behind it, so a query
    // typed to FIND this command must not also narrow its options.
    filter.value = '';
    // The model picker seeds its own highlight, since only it knows which of
    // its two steps is showing.
    highlightIndex.value = 0;
  }

  function close() {
    // Blur focused input before DOM removal — ensures focusout fires while
    // elements are still connected, so useHideOnScroll can restore the header.
    const active = document.activeElement as HTMLElement | null;
    if (active && menuRef.current?.contains(active)) {
      active.blur();
    }
    open.value = false;
    activeCommand.value = null;
    paramValues.value = {};
    filter.value = '';
    highlightIndex.value = -1;
  }

  function sendSlashCommand(cmd: string) {
    if (!isClaudeCode) return;
    close();
    sendMessage(`/${cmd}`, undefined, { useCodingAgent: true }).catch((err) => {
      showToast(`Failed to send /${cmd}: ${errorDetail(err)}`, 'error');
    });
  }

  /** Send a control request with a single option value, for a command that
   *  serves options. A *model selection* has its own path, `pickModelSelection`,
   *  because it is a pair and it records a pending pick.
   *
   *  With no live session there is nothing to send, and this command has no
   *  pending slot to park the choice in, so it only reports. */
  async function selectOption(cmd: CodingAgentCommandDef, value: string, label: string) {
    if (!threadId || !hasActiveSession.value) {
      showToast(`${cmd.label}: ${label}`, 'success');
      close();
      return;
    }
    sending.value = true;
    const result = await sendCodingAgentControl(threadId, {
      subtype: cmd.subtype,
      [cmd.params[0].key]: value,
    });
    sending.value = false;
    if (result !== 'error') showToast(`${cmd.label}: ${label}`, 'success');
    close();
  }

  /** Apply a whole *model selection*, the picker's final answer.
   *
   *  A live session is reconciled with TWO control requests. The channel
   *  preserves request order, so the model lands before the tier it accepts. If
   *  the session exits between them, the pending pick recorded above carries
   *  the tier into the next spawn or follow-up instead. */
  async function pickModelSelection(encoded: string) {
    const cmd = controlCommands.value.find(c => c.subtype === 'set_model');
    if (!cmd) return;
    const pair = decodePair(encoded);
    const label = pairLabelOf(selection.rows, encoded);
    selection.pick(encoded);

    if (!threadId || !hasActiveSession.value) {
      showToast(`${cmd.label}: ${label}`, 'success');
      close();
      return;
    }
    sending.value = true;
    const result = await sendCodingAgentControl(threadId, {
      subtype: cmd.subtype,
      [cmd.params[0].key]: pair.model,
    });
    let tierResult: 'ok' | 'pending' | 'error' | null = null;
    if (result === 'ok') currentModel.value = pair.model as CodingAgentModelValue;
    if (result === 'ok' && pair.effort) {
      const tier = pair.effort as CodingAgentReasoningEffort;
      tierResult = await sendCodingAgentControl(threadId, {
        subtype: 'set_reasoning_effort',
        effort: tier,
      });
      if (tierResult === 'ok') currentReasoningEffort.value = tier;
    }
    sending.value = false;
    if (result !== 'error' && tierResult !== 'error') {
      showToast(`${cmd.label}: ${label}`, 'success');
    }
    close();
  }

  /** The MODEL currently in force, as a label. Backend-returned per-thread
   *  values or pending overrides, so nothing leaks across threads. */
  function currentModelLabel(): string | null {
    if (!effectiveModel) return null;
    const opt = optionsOf('set_model').find(o => o.value === effectiveModel);
    return opt?.label ?? effectiveModel;
  }

  /** What a control row shows after its label. A model selection shows the
   *  whole pair, because that is what one pick sets. */
  function currentValueLabel(subtype: string): string | null {
    if (subtype !== 'set_model') return null;
    return currentModelLabel() ? selection.label : null;
  }

  async function submit() {
    const cmd = controlCommands.value.find(c => c.subtype === activeCommand.value);
    if (!cmd) return;

    const request: Record<string, string> = { subtype: cmd.subtype };
    for (const p of cmd.params) {
      const val = paramValues.value[p.key]?.trim();
      if (!val) {
        showToast(`${p.label} is required`, 'error');
        return;
      }
      request[p.key] = val;
    }

    if (!threadId || !hasActiveSession.value) {
      showToast(`${cmd.label} requires an active session`, 'error');
      close();
      return;
    }

    sending.value = true;
    const result = await sendCodingAgentControl(threadId, request);
    sending.value = false;
    if (result === 'ok') {
      showToast(`${cmd.label} sent`, 'success');
      close();
      return;
    }
    if (result === 'pending') {
      // 404: the session idle-exited between the menu render (which saw
      // `has_active_session`) and this click, so nothing was delivered and, unlike
      // `selectOption`, this form captures no pending preference to replay. Report
      // the same verdict as the pre-flight guard above rather than no-op silently.
      showToast(`${cmd.label} requires an active session`, 'error');
      close();
      return;
    }
    // 'error': sendCodingAgentControl already toasted the reason. Leave the form
    // open so the typed value survives a retry.
  }

  const cmd = controlCommands.value.find(c => c.subtype === activeCommand.value);
  const hasOptions = cmd && cmd.params.length === 1 && cmd.params[0].options?.length;
  const q = filter.value.toLowerCase();
  const offeredControl = offeredControlCommands(controlCommands.value);
  const filteredControl = q ? offeredControl.filter(c => c.label.toLowerCase().includes(q)) : offeredControl;
  const filteredBuiltin = q ? effectiveBuiltinCommands.filter(sc => sc.toLowerCase().includes(q)) : effectiveBuiltinCommands;
  const filteredSkills = q ? effectiveSkillCommands.filter(sc => sc.toLowerCase().includes(q)) : effectiveSkillCommands;

  // Flat item list for keyboard navigation
  const flatItems: ListItem[] = !cmd ? [
    ...filteredControl.map(c => ({ type: 'control' as const, subtype: c.subtype, label: c.label })),
    ...filteredBuiltin.map(sc => ({ type: 'slash' as const, name: sc })),
    ...filteredSkills.map(sc => ({ type: 'slash' as const, name: sc })),
  ] : [];

  /** The muted note beside a model's row. The Default row also names what it
   *  resolves to, so the user sees what they inherit. */
  function modelNote(row: ModelRow): string | undefined {
    const current = row.value === 'default' ? currentModelLabel() : null;
    if (!current) return row.description;
    return row.description ? `${row.description} (currently ${current})` : `Currently ${current}`;
  }

  // A model selection has its own picker, with its own steps and keyboard.
  // Every other option-bearing command (today none, but `set_permission_mode`
  // could grow options) renders as served.
  const showsModelPicker = cmd?.subtype === 'set_model';
  const optionItems: ControlOption[] = hasOptions && !showsModelPicker
    ? cmd.params[0].options!
    : [];

  function handleKeyDown(e: KeyboardEvent) {
    // The picker consumes and stops every key it owns, so anything reaching
    // here while it is up is a key neither of us handles.
    if (showsModelPicker) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      // Clear the query on the way out too. It is the option filter, and the
      // command list behind this view would read it as its own and show
      // nothing.
      if (cmd) { activeCommand.value = null; filter.value = ''; highlightIndex.value = 0; }
      else close();
      return;
    }
    if (e.key === 'Enter') {
      if (cmd && !hasOptions) {
        e.preventDefault();
        void submit();
      } else if (highlightIndex.value >= 0 && highlightIndex.value < (cmd ? optionItems.length : flatItems.length)) {
        e.preventDefault();
        if (cmd && hasOptions) {
          const opt = optionItems[highlightIndex.value];
          void selectOption(cmd, opt.value, opt.label);
        } else {
          const item = flatItems[highlightIndex.value];
          if (item.type === 'control') selectCommand(item.subtype);
          else sendSlashCommand(item.name);
        }
      }
      return;
    }
    const count = cmd ? optionItems.length : flatItems.length;
    if (!count) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlightIndex.value = highlightIndex.value < count - 1 ? highlightIndex.value + 1 : 0;
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlightIndex.value = highlightIndex.value > 0 ? highlightIndex.value - 1 : count - 1;
    }
  }

  // Sections for the list view — avoids triplicating the rendering logic
  const sections: { label: string; items: ListItem[] }[] = [];
  if (filteredControl.length > 0) sections.push({ label: 'Session', items: flatItems.filter(i => i.type === 'control') });
  if (filteredBuiltin.length > 0) sections.push({ label: 'Commands', items: filteredBuiltin.map(sc => ({ type: 'slash' as const, name: sc })) });
  if (filteredSkills.length > 0) sections.push({ label: 'Skills', items: filteredSkills.map(sc => ({ type: 'slash' as const, name: sc })) });

  function flatIndex(item: ListItem): number {
    return flatItems.findIndex(fi =>
      fi.type === item.type && (fi.type === 'control' ? fi.subtype === (item as typeof fi).subtype : fi.name === (item as typeof fi).name)
    );
  }

  return (
    <div class="control-menu" data-row-item ref={menuRef}>
      {/* Never disabled — openMenu() guards against empty commands instead.
          disabled={...} caused intermittent UX issues during Claude Code session startup races. */}
      <button
        class={`icon-btn header-icon commands-btn${hasAnyCommands(controlCommands.value, effectiveBuiltinCommands, effectiveSkillCommands) ? ' commands-btn-active' : ''}`}
        data-tooltip={`${menuLabel} controls`}
        aria-label={`${menuLabel} controls`}
        onClick={() => {
          if (open.value) { close(); return; }
          openMenu();
        }}
      >
        {isClaudeCode ? <ClaudeIcon /> : <CodexIcon />}
      </button>
      <Overlay
        open={open.value}
        onClose={close}
        anchor={menuRef.current}
        backdrop={false}
        panelClass="control-dropdown"
        panelProps={{ onKeyDown: handleKeyDown }}
      >
          {!cmd ? (
            <div class="control-list">
              <div class="control-filter-bar">
                <input
                  type="text"
                  class="control-input control-filter"
                  placeholder="Filter commands..."
                  value={filter.value}
                  ref={filterRef}
                  onInput={(e: Event) => { filter.value = (e.target as HTMLInputElement).value; highlightIndex.value = 0; }}
                />
              </div>
              {sections.map(section => (
                <Fragment key={section.label}>
                  <div class="control-section-label">{section.label}</div>
                  {section.items.map(item => {
                    const idx = flatIndex(item);
                    const label = item.type === 'control' ? item.label : `/${item.name}`;
                    const currentVal = item.type === 'control' ? currentValueLabel(item.subtype) : null;
                    const action = item.type === 'control'
                      ? () => selectCommand(item.subtype)
                      : () => sendSlashCommand(item.name);
                    return (
                      <button
                        key={label}
                        class={`control-item${idx === highlightIndex.value ? ' control-item-active' : ''}`}
                        onClick={action}
                        onMouseEnter={() => { highlightIndex.value = idx; }}
                      >
                        {label}
                        {currentVal && <span class="control-current-value"> · {currentVal}</span>}
                      </button>
                    );
                  })}
                </Fragment>
              ))}
              {sections.length === 0 && (
                <div class="control-empty">No matching commands</div>
              )}
              {/* Only on a live thread (the compose view has no worktree yet),
                  and only when nothing is being filtered, since this section is
                  not a command and would survive every search term. A thread
                  whose worktree has no frontend is not filtered out here: the
                  engine refuses by name, which is a better answer than a
                  missing button. */}
              {threadId && !q && <FrontendPreviewSection threadId={threadId} />}
            </div>
          ) : showsModelPicker ? (
            <ModelSelectionPicker
              label={cmd.label}
              selection={selection}
              disabled={sending.value}
              describeModel={modelNote}
              // Back to the command list, not out of the menu: this picker was
              // opened from a row there.
              back={{
                label: 'All commands',
                onBack: () => { activeCommand.value = null; highlightIndex.value = 0; },
              }}
              onPick={(encoded) => void pickModelSelection(encoded)}
            />
          ) : hasOptions ? (
            <ControlOptionList
              label={cmd.label}
              options={optionItems}
              currentValue={null}
              highlightIndex={highlightIndex.value}
              disabled={sending.value}
              listRef={optionsListRef}
              onPick={(opt) => void selectOption(cmd, opt.value, opt.label)}
              onHighlight={(i) => { highlightIndex.value = i; }}
            />
          ) : (
            <div class="control-form">
              <div class="control-form-title">{cmd.label}</div>
              {cmd.params.map(p => (
                <input
                  key={p.key}
                  type="text"
                  class="control-input"
                  placeholder={p.placeholder}
                  value={paramValues.value[p.key] ?? ''}
                  onInput={(e: Event) => {
                    paramValues.value = { ...paramValues.value, [p.key]: (e.target as HTMLInputElement).value };
                  }}
                  autoFocus
                />
              ))}
              <div class="control-form-actions">
                <button class="action-btn" onClick={close}>Cancel</button>
                <button class="action-btn action-btn-confirm" disabled={sending.value} onClick={submit}>
                  {sending.value ? 'Sending...' : 'Send'}
                </button>
              </div>
            </div>
          )}
      </Overlay>
    </div>
  );
}
