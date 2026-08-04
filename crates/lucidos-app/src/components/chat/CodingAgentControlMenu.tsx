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
import { isTextInput } from '../../utils/dom';
import { errorDetail } from '../../utils/errorDetail';
import { focusIfNeeded } from './promptFocus';
import { Overlay } from '../shared/Overlay';
import { availableReasoningOptions, reconcileReasoningEffort } from './codingAgentOptions';

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
  const reasoningOptions = controlCommands.value
    .find(command => command.subtype === 'set_reasoning_effort')
    ?.params[0]?.options ?? [];
  const effectiveReasoningEffort = reconcileReasoningEffort(
    selectedReasoningEffort,
    effectiveModel,
    reasoningOptions,
  ) as CodingAgentReasoningEffort | null;

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
      .catch(() => {
        // Retry both views: iOS PWA HTTP/2 connections go stale after
        // backgrounding and the first wake fetch rejects with
        // TypeError("Load failed"). The threadId guard in the empty-response
        // branch above is about "no session to wait for" — a different concern.
        if (retryCountRef.current < MAX_EMPTY_RETRIES) {
          retryCountRef.current++;
          retryTimerRef.current = window.setTimeout(loadCommands, RETRY_DELAY_MS);
        } else {
          showToast('Failed to load coding-agent commands', 'error');
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
  // Visibility check: both SplitLayout and MobileSwipeContainer render this
  // component simultaneously. Only the visible instance should consume the
  // request — the hidden one has a zero-size bounding rect.
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

  // Focus options list when entering options view
  useEffect(() => {
    optionsListRef.current?.focus();
  }, [activeCommand.value]);

  function selectCommand(subtype: string) {
    activeCommand.value = subtype;
    paramValues.value = {};
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

  /** Send a control request with a single option value (for commands with options).
   *  Always captures the selection in a pending preference — for pre-session and
   *  no-active-session this is the only path; for active sessions it's a fallback
   *  that survives the race where the live session ends between menu render and
   *  click (idle exit removes it from agent_sessions, sendCodingAgentControl returns 404).
   *  When a live session accepts the control, loadCommands() clears pending on
   *  the next refresh. */
  async function selectOption(cmd: CodingAgentCommandDef, value: string, label: string) {
    let reconciledEffort: CodingAgentReasoningEffort | null = null;
    let effortChanged = false;
    if (cmd.subtype === 'set_model') {
      const next = value === 'default' ? null : value as CodingAgentModelValue;
      reconciledEffort = reconcileReasoningEffort(
        selectedReasoningEffort,
        value,
        reasoningOptions,
      ) as CodingAgentReasoningEffort | null;
      effortChanged = selectedReasoningEffort !== null
        && reconciledEffort !== selectedReasoningEffort;
      // Compose → per-draft override (persisted via the debounced compose PUT),
      // or the PENDING slot before a draft exists; active thread → the global
      // pending signal. Never leaks across drafts and never writes a global from compose.
      if (inCompose) {
        updateComposeSelection(composeThreadId ?? null, {
          ccModel: next,
          ...(effortChanged ? { ccReasoningEffort: reconciledEffort } : {}),
        });
      } else {
        codingAgentPendingModel.value = next;
        if (effortChanged) codingAgentPendingReasoningEffort.value = reconciledEffort;
      }
    } else if (cmd.subtype === 'set_reasoning_effort') {
      const next = value as CodingAgentReasoningEffort;
      if (inCompose) updateComposeSelection(composeThreadId ?? null, { ccReasoningEffort: next });
      else codingAgentPendingReasoningEffort.value = next;
    }

    if (!threadId || !hasActiveSession.value) {
      showToast(`${cmd.label}: ${label}`, 'success');
      close();
      return;
    }
    sending.value = true;
    const request: Record<string, string> = { subtype: cmd.subtype, [cmd.params[0].key]: value };
    const result = await sendCodingAgentControl(threadId, request);
    let reconciliationResult: 'ok' | 'pending' | 'error' | null = null;
    if (result === 'ok') {
      if (cmd.subtype === 'set_model') {
        currentModel.value = value as CodingAgentModelValue;
      } else if (cmd.subtype === 'set_reasoning_effort') {
        currentReasoningEffort.value = value as CodingAgentReasoningEffort;
      }
    }
    // Keep the live runtime pair coherent too. The control channel preserves
    // request order, so the model lands before the compatible effort. If the
    // session exits between requests, the pending signal above carries the
    // reconciled value into the next spawn/follow-up instead.
    if (result === 'ok' && effortChanged && reconciledEffort !== null) {
      reconciliationResult = await sendCodingAgentControl(threadId, {
        subtype: 'set_reasoning_effort',
        effort: reconciledEffort,
      });
      if (reconciliationResult === 'ok') {
        currentReasoningEffort.value = reconciledEffort;
      }
    }
    sending.value = false;
    if (result !== 'error' && reconciliationResult !== 'error') {
      showToast(`${cmd.label}: ${label}`, 'success');
    }
    close();
  }

  /** Look up the current value for a control command and return its display label.
   *  Uses backend-returned per-thread values or pending overrides. No cross-thread leaking. */
  function currentValueLabel(subtype: string): string | null {
    const values: Record<string, string | null> = {
      set_model: effectiveModel,
      set_reasoning_effort: effectiveReasoningEffort,
    };
    const val = values[subtype];
    if (!val) return null;
    const cmd = controlCommands.value.find(c => c.subtype === subtype);
    const opt = cmd?.params[0]?.options?.find(o => o.value === val);
    return opt?.label ?? val;
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
  const filteredControl = q ? controlCommands.value.filter(c => c.label.toLowerCase().includes(q)) : controlCommands.value;
  const filteredBuiltin = q ? effectiveBuiltinCommands.filter(sc => sc.toLowerCase().includes(q)) : effectiveBuiltinCommands;
  const filteredSkills = q ? effectiveSkillCommands.filter(sc => sc.toLowerCase().includes(q)) : effectiveSkillCommands;

  // Flat item list for keyboard navigation
  const flatItems: ListItem[] = !cmd ? [
    ...filteredControl.map(c => ({ type: 'control' as const, subtype: c.subtype, label: c.label })),
    ...filteredBuiltin.map(sc => ({ type: 'slash' as const, name: sc })),
    ...filteredSkills.map(sc => ({ type: 'slash' as const, name: sc })),
  ] : [];

  const optionItems = hasOptions
    ? (cmd.subtype === 'set_reasoning_effort'
      ? availableReasoningOptions(cmd.params[0].options!, effectiveModel)
      : cmd.params[0].options!)
    : [];

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (cmd) { activeCommand.value = null; highlightIndex.value = 0; }
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
        class={`icon-btn commands-btn${hasAnyCommands(controlCommands.value, effectiveBuiltinCommands, effectiveSkillCommands) ? ' commands-btn-active' : ''}`}
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
              <input
                type="text"
                class="control-input control-filter"
                placeholder="Filter commands..."
                value={filter.value}
                ref={filterRef}
                onInput={(e: Event) => { filter.value = (e.target as HTMLInputElement).value; highlightIndex.value = 0; }}
              />
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
            </div>
          ) : hasOptions ? (
            <div class="control-list" tabIndex={0} ref={optionsListRef}>
              <div class="control-section-label">{cmd.label}</div>
              {(() => {
                const isModelCmd = cmd.subtype === 'set_model';
                const isEffortCmd = cmd.subtype === 'set_reasoning_effort';
                return optionItems.map((opt, i) => {
                const isCurrent = (isModelCmd && opt.value !== 'default' && opt.value === effectiveModel)
                  || (isEffortCmd && opt.value === effectiveReasoningEffort);
                const defaultLabel = isModelCmd && opt.value === 'default' ? currentValueLabel('set_model') : null;
                const desc = defaultLabel
                  ? `${opt.description} (currently ${defaultLabel})`
                  : opt.description;
                return (
                  <button
                    key={opt.value}
                    class={`control-item control-option${i === highlightIndex.value ? ' control-item-active' : ''}${isCurrent ? ' control-option-current' : ''}`}
                    disabled={sending.value}
                    onClick={() => void selectOption(cmd, opt.value, opt.label)}
                    onMouseEnter={() => { highlightIndex.value = i; }}
                  >
                    <span class="control-option-label">
                      {isCurrent && <span class="control-checkmark">&#10003;</span>}
                      {opt.label}
                    </span>
                    <span class="control-option-desc">{desc}</span>
                  </button>
                );
              });
              })()}
            </div>
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
