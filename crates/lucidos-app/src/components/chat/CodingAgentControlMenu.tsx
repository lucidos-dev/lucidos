import { Fragment } from 'preact';
import { useSignal, useSignalEffect, signal, untracked } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { showToast, codingAgentSessionVersion, scopeToRepoId, engineRestarting } from '../../store/store';
import { resolveScope } from '../../store/composeSelections';
import { sendMessage } from '../../store/actions/chat';
import { fetchCodingAgentCommands, type CodingAgentCommandsResponse } from '../../api/client';
import { isTextInput, keepFocusOnPress } from '../../utils/dom';
import { hasCoarsePointer, viewportIsMobile } from '../../utils/viewport';
import { errorDetail } from '../../utils/errorDetail';
import { Overlay } from '../shared/Overlay';
import { anchoredPanelStyle, useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { whenStepSettled } from '../../hooks/panelStepMorph';
import { useFocusWhenPlaced } from '../../hooks/useFocusWhenPlaced';
import { keyboardHolder, returnKeyboard } from '../shared/keyboardHandoff';
import { ControlFilter, FrontendPreviewSection, controlMenuParts } from './controlMenuParts';
import {
  codingAgentCommandsReady, codingAgentFoldOpenRequest, codingAgentMenuOpenRequest, takeControlMenuPress,
  useControlMenuCloseRequests,
} from './controlMenuRequests';
import {
  CodingAgentIcon, ControlMenuTrigger, codingAgentMenuLabel, menuCodingAgent, type CodingAgentMenuProps,
} from './ControlMenuTrigger';
import { visibleMoreTrigger } from './promptFocus';
import { usePendingOpen } from '../../hooks/usePendingOpen';
import { loadFrontendPreview } from '../../store/actions/frontend-preview';
import { failedIfFresh, loadedOr, setLoadingIfFresh, type Loadable } from '../../store/types';


/** Whether the open menu moves focus into itself. Under a finger with no
 *  keyboard up it leaves focus in the prompt: moving it would drop focus from
 *  the prompt, and the panel would jump. With the keyboard up, the filter box
 *  takes it (`keyboard`, the keyboard handoff), and so does a typed "/". */
export function menuTakesFocus(opts: { coarsePointer: boolean; typed: boolean; keyboard: boolean }): boolean {
  return opts.typed || opts.keyboard || !opts.coarsePointer;
}

/** What the last commands read answered, kept across remounts. */
interface CommandCache {
  builtin: string[];
  skill: string[];
}

const NO_COMMANDS: CommandCache = { builtin: [], skill: [] };

// Module-level: survives unmount/remount so the button appears instantly.
const commandCache = signal<Loadable<CommandCache>>({ status: 'not-loaded' });

// Max retries when commands come back empty (CC Init handshake may be in-flight)
const MAX_EMPTY_RETRIES = 10;
const RETRY_DELAY_MS = 1000;

/** True when CC has reported builtin or skill slash commands (CC binary connected). */
function hasAnyCommands(builtin: string[], skill: string[]): boolean {
  return builtin.length > 0 || skill.length > 0;
}

type ListItem = { name: string };

/** The `/` menu: a coding agent's slash commands and skills, and the frontend
 *  preview. The model and effort are not here. They are the rung control's. */
export function CodingAgentControlMenu({ threadId, composeThreadId, codingAgent, folded = false, rowAttrs }: CodingAgentMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  // Folded, the ⋯ is the only box left to anchor against.
  const foldAnchorRef = useRef<HTMLElement | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const retryTimerRef = useRef<number | null>(null);
  const retryCountRef = useRef(0);
  const openedByTypingRef = useRef(false);
  // The prompt, when it held the keyboard as the menu opened. The filter box
  // takes the keyboard from it, and closing hands it back.
  const keyboardHolderRef = useRef<HTMLElement | null>(null);
  // Only the newest `loadCommands` may apply its response. A scope or backend
  // switch fires a second fetch, and the first can land after it.
  const loadSeqRef = useRef(0);
  const pendingOpen = usePendingOpen();
  const open = useSignal(false);
  const cached = loadedOr(commandCache.peek(), NO_COMMANDS);
  const builtinCommands = useSignal<string[]>(cached.builtin);
  const skillCommands = useSignal<string[]>(cached.skill);
  const filter = useSignal('');
  const highlightIndex = useSignal(-1);
  // Active threads pass `threadId`; the compose view (a focused draft OR the
  // fresh no-draft view) does not — so `!threadId` IS "compose context".
  const inCompose = !threadId;
  const resolvedCodingAgent = menuCodingAgent(codingAgent, threadId, composeThreadId);
  const isClaudeCode = resolvedCodingAgent === 'claude-code';
  const effectiveBuiltinCommands = isClaudeCode ? builtinCommands.value : [];
  const effectiveSkillCommands = isClaudeCode ? skillCommands.value : [];
  const commandsReady = hasAnyCommands(effectiveBuiltinCommands, effectiveSkillCommands);

  function clearRetryTimer() {
    if (retryTimerRef.current !== null) {
      clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
  }

  /** An action, never a dependency: its response writes `commandCache`, so a
   *  signal it reads must not subscribe the effect that called it. */
  function loadCommands() {
    untracked(loadCommandsNow);
  }

  function loadCommandsNow() {
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
    // Compose view: the backend picker decides which commands the server
    // returns. Thread-bound menus resolve the backend server-side from
    // thread_summaries.
    const requestCodingAgent = threadId ? undefined : resolvedCodingAgent;
    const seq = ++loadSeqRef.current;
    setLoadingIfFresh(commandCache);
    fetchCodingAgentCommands(threadId, repoId, requestCodingAgent)
      .then((res: CodingAgentCommandsResponse) => {
        if (seq !== loadSeqRef.current) return;
        // The slash lists update only once the session has reported real ones.
        const slashReady = hasAnyCommands(res.builtin_commands, res.skill_commands);
        const prev = loadedOr(commandCache.value, NO_COMMANDS);
        commandCache.value = {
          status: 'loaded',
          data: {
            builtin: slashReady ? res.builtin_commands : prev.builtin,
            skill: slashReady ? res.skill_commands : prev.skill,
          },
        };
        if (slashReady) {
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
        if (seq !== loadSeqRef.current) return;
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
          commandCache.value = failedIfFresh(commandCache.value, err);
          showToast(`Failed to load coding-agent commands: ${errorDetail(err)}`, 'error');
        }
      });
  }

  /** The box the panel opens against: the menu's own, or the ⋯ while folded. */
  function anchorBox(): HTMLElement | null {
    if (!folded) return menuRef.current;
    if (!foldAnchorRef.current?.isConnected) foldAnchorRef.current = visibleMoreTrigger();
    return foldAnchorRef.current;
  }

  /** Open the command menu with an optional filter. `typed` is true when a
   *  typed "/" opened it, so the typing continues in the filter. */
  function openMenu(filterText = '', typed = false) {
    if (!hasAnyCommands(effectiveBuiltinCommands, effectiveSkillCommands)) return;
    const trigger = anchorBox();
    if (!trigger) return;
    // Read at the press: the holder is whatever had the keyboard then.
    const holder = keyboardHolder();
    pendingOpen.open(controlMenuParts, () => {
      openedByTypingRef.current = typed;
      keyboardHolderRef.current = holder;
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
    }, trigger);
  }

  useEffect(() => {
    retryCountRef.current = 0;
    // The compose view (no threadId) is loaded by the `composeRepoId` effect
    // below. Loading here too would double-fetch on mount.
    if (threadId) loadCommands();
    return clearRetryTimer;
  }, [threadId, resolvedCodingAgent]);

  // Blur before unmount so useHideOnScroll's focusout handler fires while
  // the element is still connected, properly resetting keyboardOpen.
  useEffect(() => close, []);

  // A press that landed on the placeholder before this code arrived. With no
  // commands yet it opens nothing, exactly as a press on this button would.
  useEffect(() => {
    if (takeControlMenuPress('coding-agent')) openMenu();
  }, []);

  // A fold or unfold moves the anchor to another box. An open panel is shut
  // then, not left pinned to the box that went.
  const wasFolded = useRef(folded);
  useEffect(() => {
    if (wasFolded.current !== folded) close();
    wasFolded.current = folded;
  }, [folded]);
  useControlMenuCloseRequests(open, close);

  useEffect(() => {
    codingAgentCommandsReady.value = commandsReady;
  }, [commandsReady]);
  useEffect(() => () => { codingAgentCommandsReady.value = false; }, []);

  // The ⋯ menu row for a folded menu asks for the panel against the ⋯. The ⋯
  // has closed by then, so a tap with nothing to open says why.
  useSignalEffect(() => {
    const anchor = codingAgentFoldOpenRequest.value;
    if (!folded || !anchor) return;
    codingAgentFoldOpenRequest.value = null;
    if (!untracked(() => codingAgentCommandsReady.value)) {
      showToast('Claude Code commands are still loading', 'info');
      return;
    }
    foldAnchorRef.current = anchor;
    openMenu();
  });

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

  // Re-fetch whenever the compose view's commands could differ: entering
  // compose, a repo pick, a backend pick, or focusing another draft. Clear the
  // module-level cache first so the previous scope's skills don't flash before
  // the new fetch resolves. Thread-bound menus take the thread's repo instead.
  //
  // Keyed on what the request sends, read in render, so a prop change counts
  // too. Never a signal effect: it re-runs on signals only, and the menu stays
  // mounted across a draft switch.
  const composeRepoId = inCompose ? (scopeToRepoId(resolveScope(composeThreadId)) ?? '') : null;
  useEffect(() => {
    if (composeRepoId === null) return;
    commandCache.value = { status: 'not-loaded' };
    builtinCommands.value = [];
    skillCommands.value = [];
    retryCountRef.current = 0;
    loadCommands();
  }, [composeRepoId, resolvedCodingAgent]);

  // Open from PromptInput when user types "/" prefix.
  // Must use useSignalEffect (not useEffect) — @preact/signals can optimize
  // away re-renders when a signal doesn't affect DOM output, which prevents
  // useEffect deps from being re-evaluated.
  // Layout check: `App` mounts only the active layout's pane tree (desktop XOR
  // mobile), so exactly one instance of this menu exists. It can still be
  // unlaid-out on the frame the request arrives (zero-size bounding rect), and
  // the menu anchors its overlay to that box, so wait for a real one. Folded,
  // that box is the ⋯.
  useSignalEffect(() => {
    const req = codingAgentMenuOpenRequest.value;
    if (req !== null) {
      const laidOut = (el: HTMLElement | null) => el !== null && el.getBoundingClientRect().width > 0;
      if (!laidOut(anchorBox())) {
        // Layout may not have finalized yet, so retry after the next frame.
        requestAnimationFrame(() => {
          if (codingAgentMenuOpenRequest.value !== null && laidOut(anchorBox())) {
            codingAgentMenuOpenRequest.value = null;
            openMenu(req, true);
          }
        });
        return;
      }
      codingAgentMenuOpenRequest.value = null;
      openMenu(req, true);
    }
  });

  // Global "/" shortcut to open dropdown when not typing in an input. The
  // listener mounts once, so it reads `openMenu` through a ref. The menu
  // outlives a thread switch, and without the ref the first render's closure
  // would fetch the commands of the thread that was open then.
  const openMenuRef = useRef(openMenu);
  openMenuRef.current = openMenu;
  useEffect(() => {
    function handleSlash(e: KeyboardEvent) {
      if (open.value) return;
      if (isTextInput(e.target)) return;
      // A modified slash belongs to the shortcut registry (⌘/ shows shortcuts).
      if (e.key === '/' && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        openMenuRef.current('', true);
      }
    }
    document.addEventListener('keydown', handleSlash);
    return () => document.removeEventListener('keydown', handleSlash);
  }, []);

  // Scroll highlighted item into view
  useEffect(() => {
    if (highlightIndex.value < 0) return;
    const el = panelRef.current?.querySelector('.control-item-active');
    whenStepSettled(el, () => el?.scrollIntoView({ block: 'nearest' }));
  }, [highlightIndex.value]);

  // A phone gives the panel the header palette width (.control-dropdown), so
  // it centres in the pane rather than hanging from its button.
  const anchorEl = folded ? foldAnchorRef.current : menuRef.current;
  const pos = useAnchoredPosition(
    open.value ? anchorEl : null,
    panelRef,
    '.thread-pane',
    viewportIsMobile.value ? 'container-center' : 'start',
  );

  // Focus the filter box on open. Under a finger with no keyboard up it leaves
  // focus in the prompt: moving it would drop focus from the prompt, and the
  // panel would jump.
  useFocusWhenPlaced(
    () => (open.value && menuTakesFocus({
      coarsePointer: hasCoarsePointer(), typed: openedByTypingRef.current,
      keyboard: keyboardHolderRef.current !== null,
    }) ? filterRef.current : null),
    [open.value],
  );

  function close() {
    returnKeyboard(keyboardHolderRef.current, panelRef.current);
    keyboardHolderRef.current = null;
    // Blur focused input before DOM removal — ensures focusout fires while
    // elements are still connected, so useHideOnScroll can restore the header.
    const active = document.activeElement as HTMLElement | null;
    if (active && (menuRef.current?.contains(active) || panelRef.current?.contains(active))) {
      active.blur();
    }
    open.value = false;
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

  const q = filter.value.toLowerCase();
  const filteredBuiltin = q ? effectiveBuiltinCommands.filter(sc => sc.toLowerCase().includes(q)) : effectiveBuiltinCommands;
  const filteredSkills = q ? effectiveSkillCommands.filter(sc => sc.toLowerCase().includes(q)) : effectiveSkillCommands;

  // Flat item list for keyboard navigation
  const flatItems: ListItem[] = [...filteredBuiltin, ...filteredSkills].map(name => ({ name }));

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
      return;
    }
    if (e.key === 'Enter') {
      if (highlightIndex.value >= 0 && highlightIndex.value < flatItems.length) {
        e.preventDefault();
        sendSlashCommand(flatItems[highlightIndex.value].name);
      }
      return;
    }
    const count = flatItems.length;
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

  // Sections for the list view — avoids duplicating the rendering logic
  const sections: { label: string; items: ListItem[] }[] = [];
  if (filteredBuiltin.length > 0) sections.push({ label: 'Commands', items: filteredBuiltin.map(name => ({ name })) });
  if (filteredSkills.length > 0) sections.push({ label: 'Skills', items: filteredSkills.map(name => ({ name })) });

  function flatIndex(item: ListItem): number {
    return flatItems.findIndex(fi => fi.name === item.name);
  }

  return (
    <>
      {!folded && (
        <div class="control-menu" {...rowAttrs} ref={menuRef}>
          {/* Never disabled: openMenu() guards against empty commands instead.
              disabled={...} caused intermittent UX issues during Claude Code session startup races. */}
          <ControlMenuTrigger
            label={codingAgentMenuLabel(resolvedCodingAgent)}
            extraClass={commandsReady ? 'commands-btn-active' : undefined}
            onClick={() => {
              if (open.value) { close(); return; }
              if (pendingOpen.takeBack()) return;
              openMenu();
            }}
          >
            <CodingAgentIcon agent={resolvedCodingAgent} />
          </ControlMenuTrigger>
        </div>
      )}
      <Overlay
        open={open.value}
        onClose={close}
        anchor={anchorEl}
        backdrop={false}
        // Portaled so it can lay over the header: a mobile swipe pane clips at
        // its own edge and caps any z-index inside it.
        portal
        panelClass="surface-box control-dropdown"
        panelRef={panelRef}
        panelStyle={anchoredPanelStyle(pos)}
        panelProps={{ onKeyDown: handleKeyDown }}
      >
        <div class="control-list" data-surface-step="commands">
          <ControlFilter
            placeholder="Filter commands…"
            value={filter.value}
            inputRef={filterRef}
            onInput={(value) => {
              filter.value = value;
              highlightIndex.value = 0;
            }}
          />
          {sections.map(section => (
            <Fragment key={section.label}>
              <div class="control-section-label">{section.label}</div>
              {section.items.map(item => {
                const idx = flatIndex(item);
                return (
                  <button
                    key={item.name}
                    class={`control-item${idx === highlightIndex.value ? ' control-item-active' : ''}`}
                    onMouseDown={keepFocusOnPress}
                    onClick={() => sendSlashCommand(item.name)}
                    onMouseEnter={() => { highlightIndex.value = idx; }}
                  >
                    {`/${item.name}`}
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
      </Overlay>
    </>
  );
}
