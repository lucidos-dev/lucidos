import { openAppSearch } from '../store/actions/apps';
import { useEffect } from 'preact/hooks';
import { unfocusThread } from '../store/actions/threads';
import { focusPromptNow, openAgentMenu } from '../components/chat/promptFocus';
import { activeMenuItem, panelOverlay, searchEverywhereOpen, searchEverywhereAnchor, focusedPane, focusedThreadId, toggleExchangeCollapsed, toggleInitiatorCollapsed } from '../store/store';
import { isInteractiveTarget, isTextInput, isThreadTranscript } from '../utils/dom';
import { dismissTopOverlay, overlayStack, topPanelOverlay } from '../store/overlayStack';
import { nativeFullscreenElement } from '../store/appFullscreenHost';
import { runCloseCascade } from '../store/actions/threadActions';
import { matchShortcut } from '../store/actions/keybindings';
import { shortcutDef, type ShortcutId } from '../utils/shortcuts';
import { pressCallToggleIfShown } from '../components/chat/PromptRowControls';
import { toggleFollowLiveEdge } from '../components/chat/FollowToggle';
import { showFocusedThreadDiff, applyFocusedThreadChange } from '../components/chat/WaitingBanner';
import { toggleAppFullscreenIfShown, toggleSourceView, toggleLineWrap } from '../components/layout/ContentHeaderActions';
import { appFrameFor, isCanvasAppFrame } from '../utils/appFrame';
import { findAvailable, findSurface, focusedFindSurface, toggleFocusedFind } from '../store/actions/find-bar';
import { adjustUiScale, resetUiScale, scaleModalOpen, dismissScaleModal } from '../components/shared/scaleModalState';
import { UI_SCALE_STEP } from '../store/actions/preferences';
import { isMobile } from '../utils/viewport';
import { isMac } from '../utils/platform';
import {
  toggleThreadPane, toggleContentPane,
  focusOrToggleThreadDrawer, toggleMaximizeFocusedPaneGroup,
  stepThreadPaneWidth, stepThreadDrawerWidth, resetPaneLayout, revealThreadPane,
  toggleThreadFilter, startThreadSearch,
} from '../store/actions/pane';
import { toggleMenuDrawer } from '../components/layout/ContentNav';
import { refreshPanelIfLive } from '../components/layout/RefreshIndicator';
import { switchMenuItem, openSettingsSubview } from '../store/actions/menu';
import { openFileSearch } from '../components/files/fileSearchActions';
import { focusNewestToast } from '../components/shared/Toast';
import { handleOverlayTab } from '../components/shared/overlayFocus';
import { promptRenameThread } from '../store/actions/threadRename';
import { copyLastResponse } from '../components/chat/copyLastResponse';
import { promptStopRequested, promptSideQuestionRequested } from '../components/chat/prompt-input-helpers';
import { seedDrawerHighlight, openHighlightedThreadActions, toggleFocusedThreadFamily, stepFocusedThreadInList } from '../components/drawer/ThreadDrawer';
import { openThreadTitleMenu } from '../components/chat/ThreadTitle';
import { focusIntoPane, handlePaneTab, reconcilePaneFocus } from '../components/layout/paneFocus';
import { historyBack, historyForward } from '../store/actions/focused-pane-history';
import { stepThreadTurn, parseNavigatedTurn } from '../components/chat/scrollState';
import { stepViewedNotification } from '../store/actions/notifications';
import { navFocusElement, applyNavFocus } from '../components/shared/focusMarker';

function startNewThread() {
  unfocusThread();
  requestAnimationFrame(() => {
    focusPromptNow();
  });
}

function focusInThreadPane(focus: () => void): void {
  revealThreadPane();
  requestAnimationFrame(focus);
}

function toggleNavigatedTurnCollapsed(): boolean {
  const el = navFocusElement();
  if (!el || typeof el.matches !== 'function' || !el.matches('.chat-exchange')) return false;
  const parsed = parseNavigatedTurn(
    el.dataset.threadId ?? null,
    el.dataset.userSeq ?? null,
    el.dataset.collapseKind ?? null,
  );
  if (!parsed) return false;
  if (parsed.kind === 'response') toggleExchangeCollapsed(parsed.threadId, parsed.userSeq);
  else toggleInitiatorCollapsed(parsed.threadId, parsed.userSeq);
  applyNavFocus(el);
  return true;
}

function toggleSearchEverywhere(): void {
  if (!searchEverywhereOpen.value) searchEverywhereAnchor.value = null;
  searchEverywhereOpen.value = !searchEverywhereOpen.value;
}

const SHORTCUT_ACTIONS: Record<ShortcutId, () => void> = {
  newThread: startNewThread,
  closeThread: () => void runCloseCascade(),
  searchEverywhere: toggleSearchEverywhere,
  searchFiles: () => openFileSearch(),
  searchApps: openAppSearch,
  openSettings: () => switchMenuItem('settings'),
  showShortcuts: () => openSettingsSubview('keyboard-shortcuts'),
  openNotifications: () => switchMenuItem('notifications'),
  focusNewestToast,
  focusComposer: () => focusInThreadPane(focusPromptNow),
  stopThread: () => { promptStopRequested.value = true; },
  askSideQuestion: () => {
    revealThreadPane();
    promptSideQuestionRequested.value = true;
  },
  copyLastResponse,
  renameThread: () => {
    const id = focusedThreadId.value;
    if (id) void promptRenameThread(id);
  },
  followLiveEdge: toggleFollowLiveEdge,
  toggleCall: pressCallToggleIfShown,
  openAgentMenu: () => focusInThreadPane(openAgentMenu),
  showThreadDiff: showFocusedThreadDiff,
  applyChange: () => { if (overlayStack.value.length === 0) applyFocusedThreadChange(); },
  openThreadActions: () => {
    if (openHighlightedThreadActions()) return;
    if (focusedThreadId.value) focusInThreadPane(openThreadTitleMenu);
  },
  toggleSubthreads: toggleFocusedThreadFamily,
  toggleThreadFilter,
  searchThreads: startThreadSearch,
  toggleMenuDrawer,
  historyBack: () => historyBack(),
  historyForward: () => historyForward(),
  prevThreadInList: () => stepFocusedThreadInList(-1),
  nextThreadInList: () => stepFocusedThreadInList(1),
  prevTurnOrNotification: () => { if (!stepViewedNotification(-1)) focusIntoPane(stepThreadTurn(-1)); },
  nextTurnOrNotification: () => { if (!stepViewedNotification(1)) focusIntoPane(stepThreadTurn(1)); },
  toggleThreadDrawer: () => { if (focusOrToggleThreadDrawer()) seedDrawerHighlight(); },
  toggleThreadPane,
  toggleContentPane,
  maximizePaneGroup: toggleMaximizeFocusedPaneGroup,
  narrowThreadPane: () => stepThreadPaneWidth(-1),
  widenThreadPane: () => stepThreadPaneWidth(1),
  narrowThreadDrawer: () => stepThreadDrawerWidth(-1),
  widenThreadDrawer: () => stepThreadDrawerWidth(1),
  resetPaneLayout,
  refreshPanel: refreshPanelIfLive,
  toggleAppFullscreen: toggleAppFullscreenIfShown,
  findInView: toggleFocusedFind,
  toggleSourceView,
  toggleLineWrap,
  zoomIn: () => adjustUiScale(UI_SCALE_STEP),
  zoomOut: () => adjustUiScale(-UI_SCALE_STEP),
  zoomReset: () => resetUiScale(),
};

export function dispatchEscape(
  active: Element | null,
  nativeFullscreen: boolean = nativeFullscreenElement() !== null,
): 'dismissed' | 'self-managed' | 'blurred' | 'noop' | 'fullscreen' {
  if (nativeFullscreen) return 'fullscreen';
  if (dismissTopOverlay()) return 'dismissed';
  if (isTextInput(active)) {
    if ((active as HTMLElement).hasAttribute?.('data-escape-self')) return 'self-managed';
    (active as HTMLElement).blur();
    return 'blurred';
  }
  return 'noop';
}

const FORWARDED_KEYDOWN_TYPE = 'lucidos:keydown';

type ChordLike = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>;

export function shouldTypeToFocusPrompt(
  e: Pick<KeyboardEvent, 'isComposing' | 'metaKey' | 'ctrlKey' | 'altKey' | 'key' | 'target'>,
  opts: { mobile: boolean; overlayOpen: boolean },
): boolean {
  return !opts.mobile
    && !opts.overlayOpen
    && !e.isComposing
    && !isTextInput(e.target)
    && !e.metaKey && !e.ctrlKey && !e.altKey
    && e.key.length === 1
    && !(e.key === ' ' && (isThreadTranscript(e.target) || isInteractiveTarget(e.target)));
}

export function isMacTextEditingKey(
  e: Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key' | 'target'>,
  mac: boolean = isMac,
): boolean {
  if (!mac || e.metaKey || e.shiftKey || !isTextInput(e.target)) return false;
  const ctrlLetter = e.ctrlKey && !e.altKey && /^[a-z]$/i.test(e.key);
  const optionArrow = e.altKey && !e.ctrlKey && e.key.startsWith('Arrow');
  return ctrlLetter || optionArrow;
}

function escapePolicyHasWork(): boolean {
  if (overlayStack.value.length > 0 || nativeFullscreenElement() !== null) return true;
  const active = typeof document === 'undefined' ? null : document.activeElement;
  return active?.hasAttribute?.('data-escape-self') === true;
}

const CLAIMS_ONLY_WHEN: Partial<Record<ShortcutId, (inContent: boolean) => boolean>> = {
  searchApps: () => topPanelOverlay() === null && panelOverlay.value === null
    && (activeMenuItem.value === 'apps' || activeMenuItem.value === 'plugins'),
  findInView: (inContent) => topPanelOverlay() === null
    && (findSurface.value !== null
      || (inContent ? findAvailable('content') : focusedFindSurface() !== null)),
};

function claimsChord(id: ShortcutId, inContent: boolean): boolean {
  return CLAIMS_ONLY_WHEN[id]?.(inContent) ?? true;
}

export function classifyChord(chord: ChordLike, inContent = false): ShortcutId | 'escape' | null {
  if (chord.key === 'Escape' && escapePolicyHasWork()) return 'escape';
  const id = matchShortcut(chord);
  if (id && claimsChord(id, inContent)) return id;
  if (chord.key === 'Escape') return 'escape';
  return null;
}

export function dispatchForwardedChord(chord: ChordLike, pane: 'content' | 'thread' = 'content'): void {
  const result = classifyChord(chord, pane === 'content');
  if (result === null) return;
  if (result !== 'escape' && shortcutDef(result).hostOnly) return;
  focusedPane.value = pane;
  if (result === 'escape') {
    dispatchEscape(document.activeElement);
  } else {
    SHORTCUT_ACTIONS[result]();
  }
}

export function dispatchPreviewIframeShortcut(e: KeyboardEvent): boolean {
  const route = classifyChord(e, true);
  if (route !== null && route !== 'escape') {
    e.preventDefault();
    focusedPane.value = 'content';
    SHORTCUT_ACTIONS[route]();
    return true;
  }
  if (route === 'escape') {
    const result = dispatchEscape(document.activeElement);
    if (result === 'dismissed' || result === 'blurred') e.preventDefault();
    return result === 'dismissed' || result === 'blurred' || result === 'self-managed';
  }
  return false;
}

export function useKeyboardShortcuts(): void {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const route = isMacTextEditingKey(e) ? null : classifyChord(e);
      if (route !== null && route !== 'escape') {
        e.preventDefault();
        SHORTCUT_ACTIONS[route]();
        return;
      }

      if (
        e.key === 'Enter' &&
        !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey &&
        (e.target as HTMLElement | null)?.matches?.('.thread-content') &&
        toggleNavigatedTurnCollapsed()
      ) {
        e.preventDefault();
        return;
      }

      if (e.key === 'Tab') {
        if (e.defaultPrevented || handleOverlayTab(e)) return;
        if (handlePaneTab(e)) e.preventDefault();
        return;
      }

      if (shouldTypeToFocusPrompt(e, { mobile: isMobile(), overlayOpen: overlayStack.value.length > 0 })) {
        focusPromptNow();
        e.preventDefault();
        document.execCommand('insertText', false, e.key);
      }
    }

    function handleKeyUp(e: KeyboardEvent) {
      if ((e.key === 'Meta' || e.key === 'Control') && scaleModalOpen.value) {
        dismissScaleModal();
      }
    }

    function handleEscapeCapture(e: KeyboardEvent) {
      if (e.key !== 'Escape') return;
      const route = classifyChord(e);
      if (route !== null && route !== 'escape') {
        e.preventDefault();
        e.stopPropagation();
        SHORTCUT_ACTIONS[route]();
        return;
      }
      const result = dispatchEscape(document.activeElement);
      if (result === 'dismissed') {
        e.preventDefault();
        e.stopPropagation();
      } else if (result === 'fullscreen') {
        e.stopPropagation();
      } else if (result === 'blurred') {
        e.preventDefault();
        reconcilePaneFocus(focusedPane.value);
      }
    }

    function handleForwardedKeydown(e: MessageEvent) {
      const data = e.data as {
        type?: unknown; key?: unknown;
        metaKey?: unknown; ctrlKey?: unknown; shiftKey?: unknown; altKey?: unknown;
      } | null;
      if (!data || typeof data !== 'object' || data.type !== FORWARDED_KEYDOWN_TYPE) return;
      if (typeof data.key !== 'string') return;
      const frame = appFrameFor(e.source);
      if (!frame) return;
      dispatchForwardedChord({
        key: data.key,
        metaKey: data.metaKey === true,
        ctrlKey: data.ctrlKey === true,
        shiftKey: data.shiftKey === true,
        altKey: data.altKey === true,
      }, isCanvasAppFrame(frame) ? 'content' : 'thread');
    }

    document.addEventListener('keydown', handleKeyDown);
    document.addEventListener('keyup', handleKeyUp);
    document.addEventListener('keydown', handleEscapeCapture, true);
    window.addEventListener('message', handleForwardedKeydown);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      document.removeEventListener('keyup', handleKeyUp);
      document.removeEventListener('keydown', handleEscapeCapture, true);
      window.removeEventListener('message', handleForwardedKeydown);
    };
  }, []);
}
