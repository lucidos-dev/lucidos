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
import { filterSlashCommands, slashCommandText } from './slashCommandFilter';
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


export function menuTakesFocus(opts: { coarsePointer: boolean; typed: boolean; keyboard: boolean }): boolean {
  return opts.typed || opts.keyboard || !opts.coarsePointer;
}

interface CommandCache {
  builtin: string[];
  skill: string[];
}

const NO_COMMANDS: CommandCache = { builtin: [], skill: [] };

const commandCache = signal<Loadable<CommandCache>>({ status: 'not-loaded' });

const MAX_EMPTY_RETRIES = 10;
const RETRY_DELAY_MS = 1000;

function hasAnyCommands(builtin: string[], skill: string[]): boolean {
  return builtin.length > 0 || skill.length > 0;
}

type ListItem = { name: string };

export function CodingAgentControlMenu({ threadId, composeThreadId, codingAgent, folded = false, rowAttrs }: CodingAgentMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const foldAnchorRef = useRef<HTMLElement | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const retryTimerRef = useRef<number | null>(null);
  const retryCountRef = useRef(0);
  const openedByTypingRef = useRef(false);
  const keyboardHolderRef = useRef<HTMLElement | null>(null);
  const loadSeqRef = useRef(0);
  const pendingOpen = usePendingOpen();
  const open = useSignal(false);
  const cached = loadedOr(commandCache.peek(), NO_COMMANDS);
  const builtinCommands = useSignal<string[]>(cached.builtin);
  const skillCommands = useSignal<string[]>(cached.skill);
  const filter = useSignal('');
  const highlightIndex = useSignal(-1);
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

  function loadCommands() {
    untracked(loadCommandsNow);
  }

  function loadCommandsNow() {
    clearRetryTimer();
    if (engineRestarting.value) return;
    const repoId = threadId ? undefined : (scopeToRepoId(resolveScope(composeThreadId)) ?? '');
    const requestCodingAgent = threadId ? undefined : resolvedCodingAgent;
    const seq = ++loadSeqRef.current;
    setLoadingIfFresh(commandCache);
    fetchCodingAgentCommands(threadId, repoId, requestCodingAgent)
      .then((res: CodingAgentCommandsResponse) => {
        if (seq !== loadSeqRef.current) return;
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
          if (isClaudeCode && threadId && retryCountRef.current < MAX_EMPTY_RETRIES) {
            retryCountRef.current++;
            retryTimerRef.current = window.setTimeout(loadCommands, RETRY_DELAY_MS);
          }
        }
      })
      .catch((err: unknown) => {
        if (seq !== loadSeqRef.current) return;
        if (retryCountRef.current < MAX_EMPTY_RETRIES) {
          retryCountRef.current++;
          retryTimerRef.current = window.setTimeout(loadCommands, RETRY_DELAY_MS);
        } else {
          commandCache.value = failedIfFresh(commandCache.value, err);
          showToast(`Failed to load coding-agent commands: ${errorDetail(err)}`, 'error');
        }
      });
  }

  function anchorBox(): HTMLElement | null {
    if (!folded) return menuRef.current;
    if (!foldAnchorRef.current?.isConnected) foldAnchorRef.current = visibleMoreTrigger();
    return foldAnchorRef.current;
  }

  function openMenu(filterText = '', typed = false) {
    if (!hasAnyCommands(effectiveBuiltinCommands, effectiveSkillCommands)) return;
    const trigger = anchorBox();
    if (!trigger) return;
    const holder = keyboardHolder();
    pendingOpen.open(controlMenuParts, () => {
      openedByTypingRef.current = typed;
      keyboardHolderRef.current = holder;
      open.value = true;
      filter.value = filterText;
      highlightIndex.value = 0;
      retryCountRef.current = 0;
      loadCommands();
      if (threadId) void loadFrontendPreview();
    }, trigger);
  }

  useEffect(() => {
    retryCountRef.current = 0;
    if (threadId) loadCommands();
    return clearRetryTimer;
  }, [threadId, resolvedCodingAgent]);

  useEffect(() => close, []);

  useEffect(() => {
    if (takeControlMenuPress('coding-agent')) openMenu();
  }, []);

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

  useSignalEffect(() => {
    if (codingAgentSessionVersion.value > 0) {
      retryCountRef.current = 0;
      loadCommands();
    }
  });

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

  const composeRepoId = inCompose ? (scopeToRepoId(resolveScope(composeThreadId)) ?? '') : null;
  useEffect(() => {
    if (composeRepoId === null) return;
    commandCache.value = { status: 'not-loaded' };
    builtinCommands.value = [];
    skillCommands.value = [];
    retryCountRef.current = 0;
    loadCommands();
  }, [composeRepoId, resolvedCodingAgent]);

  useSignalEffect(() => {
    const req = codingAgentMenuOpenRequest.value;
    if (req !== null) {
      const laidOut = (el: HTMLElement | null) => el !== null && el.getBoundingClientRect().width > 0;
      if (!laidOut(anchorBox())) {
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

  const openMenuRef = useRef(openMenu);
  openMenuRef.current = openMenu;
  useEffect(() => {
    function handleSlash(e: KeyboardEvent) {
      if (open.value) return;
      if (isTextInput(e.target)) return;
      if (e.key === '/' && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        openMenuRef.current('', true);
      }
    }
    document.addEventListener('keydown', handleSlash);
    return () => document.removeEventListener('keydown', handleSlash);
  }, []);

  useEffect(() => {
    if (highlightIndex.value < 0) return;
    const el = panelRef.current?.querySelector('.control-item-active');
    whenStepSettled(el, () => el?.scrollIntoView({ block: 'nearest' }));
  }, [highlightIndex.value]);

  const anchorEl = folded ? foldAnchorRef.current : menuRef.current;
  const pos = useAnchoredPosition(
    open.value ? anchorEl : null,
    panelRef,
    '.thread-pane',
    viewportIsMobile.value ? 'container-center' : 'start',
  );

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
    const text = slashCommandText(cmd, filter.value);
    close();
    sendMessage(text, undefined, { useCodingAgent: true }).catch((err) => {
      showToast(`Failed to send /${cmd}: ${errorDetail(err)}`, 'error');
    });
  }

  const filteredBuiltin = filterSlashCommands(effectiveBuiltinCommands, filter.value);
  const filteredSkills = filterSlashCommands(effectiveSkillCommands, filter.value);

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
          {threadId && !filter.value &&<FrontendPreviewSection threadId={threadId} />}
        </div>
      </Overlay>
    </>
  );
}
