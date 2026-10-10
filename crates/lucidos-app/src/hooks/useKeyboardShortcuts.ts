import { useEffect } from 'preact/hooks';
import { unfocusThread } from '../store/actions/threads';
import { focusPromptNow, openAgentMenu } from '../components/chat/promptFocus';
import { searchEverywhereOpen, searchEverywhereAnchor, focusedPane, focusedThreadId, toggleExchangeCollapsed, toggleInitiatorCollapsed } from '../store/store';
import { isTextInput, isThreadTranscript } from '../utils/dom';
import { dismissTopOverlay, overlayStack, topPanelOverlay } from '../store/overlayStack';
import { nativeFullscreenElement } from '../store/appFullscreenHost';
import { runCloseCascade } from '../store/actions/threadActions';
import { matchShortcut } from '../store/actions/keybindings';
import { shortcutDef, type ShortcutId } from '../utils/shortcuts';
import { toggleFollowLiveEdge, pressCallToggleIfShown } from '../components/chat/PromptRowControls';
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

/** Reveal the thread pane, then run `focus` once it is laid out. A collapsed
 *  pane holds its prompt row at zero size, where a focus or a press would land
 *  on nothing the user can see. */
function focusInThreadPane(focus: () => void): void {
  revealThreadPane();
  requestAnimationFrame(focus);
}

/** Toggle the expand/collapse of the turn currently carrying the navigation focus
 *  marker — the ⌘↑/⌘↓-highlighted `.chat-exchange`. The Enter counterpart to
 *  clicking a turn's response/initiator header: it folds the response body when the
 *  turn has one, else the initiator panel (a response-less divider / change turn).
 *  RE-ASSERTS the marker so it survives the Enter
 *  keydown's clear (any keydown retires the marker's ref) and repeated Enter presses stay
 *  visible and reversible. Lives here (not in `scrollState`) so `scrollState` stays
 *  free of the heavy `store` import — see `parseNavigatedTurn`'s doc. Returns false
 *  (leaving the keystroke unconsumed) when no turn is marked, the marker is on a
 *  non-turn element (a settings / plugin landing), or the turn is not collapsible. */
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
  // Re-stick the marker: this runs from the Enter keydown's bubble phase, after
  // focusMarker's capture-phase keydown listener has already handled that keydown.
  // Inside the marker's hold that means it banked the dismissal and retired the ref;
  // after the hold it means the dissolve has started. Re-applying supersedes either,
  // keeping the highlight on the turn for the next toggle. A later scroll / click /
  // other keypress dismisses it normally.
  applyNavFocus(el);
  return true;
}

/** A shortcut opens the palette with no button behind it. So it drops the
 *  anchor a past button open left, and the palette follows the focused pane. */
function toggleSearchEverywhere(): void {
  if (!searchEverywhereOpen.value) searchEverywhereAnchor.value = null;
  searchEverywhereOpen.value = !searchEverywhereOpen.value;
}

/** What each registry shortcut does when its (current, possibly-customized)
 *  binding fires. The registry (`utils/shortcuts.ts`) owns the keys; this map
 *  owns the behavior — so rebinding a shortcut in Settings takes effect here
 *  with no code change. */
const SHORTCUT_ACTIONS: Record<ShortcutId, () => void> = {
  newThread: startNewThread,
  closeThread: () => void runCloseCascade(),
  searchEverywhere: toggleSearchEverywhere,
  searchFiles: () => openFileSearch(),
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
  // Never behind an open dialog: a second press while Apply's own confirm is
  // up would stack another one.
  applyChange: () => { if (overlayStack.value.length === 0) applyFocusedThreadChange(); },
  // A thread row highlighted in the focused drawer wins. Otherwise the open
  // thread's own menu opens, from its title.
  openThreadActions: () => {
    if (openHighlightedThreadActions()) return;
    if (focusedThreadId.value) focusInThreadPane(openThreadTitleMenu);
  },
  // Toggles the OPEN (focused) thread's sub-thread family in the drawer — works
  // from any pane (no drawer-focus gate); no-op when the focused thread has no
  // sub-threads.
  toggleSubthreads: toggleFocusedThreadFamily,
  toggleThreadFilter,
  searchThreads: startThreadSearch,
  toggleMenuDrawer,
  historyBack: () => historyBack(),
  historyForward: () => historyForward(),
  prevThreadInList: () => stepFocusedThreadInList(-1),
  nextThreadInList: () => stepFocusedThreadInList(1),
  // Step the open notification newer/older when the content pane holds one.
  // Otherwise step the transcript one turn (a .chat-exchange) up/down and land
  // focus in it, so continuous Arrow/Page scrolling follows.
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

/** Non-destructive Escape policy, in priority order:
 *  0. while an element is NATIVELY fullscreen, stand down entirely: the browser
 *     takes this Escape to exit fullscreen and no handler can stop it (Escape is
 *     excluded from the events that grant the user activation a re-request would
 *     need, and the only way to consume the close request first is the top
 *     layer, which the overlay layer deliberately does not use). Acting anyway
 *     would make one Escape do two things: close the overlay AND drop the app
 *     out of fullscreen. Standing down leaves it doing one, and the overlay is
 *     then plainly visible in the normal layout for the next Escape to close.
 *  1. dismiss the top registered overlay (modal / confirm / pseudo-fullscreen),
 *  2. else, if the focused text input manages its own Escape (`data-escape-self`),
 *     leave focus alone so its keydown handler can run. Inputs whose blur
 *     commits work use it, such as the trigger group rename field: there a
 *     blur-on-Escape would SAVE instead of cancel,
 *  3. else blur a focused text input (the universal "Esc defocuses" gesture),
 *  4. else no-op — Escape NEVER touches the focused thread or discards work.
 *  Returns which branch fired so the caller can preventDefault/stopPropagation
 *  appropriately. The stand-down has its own value, `'fullscreen'`, because it
 *  is NOT the same as `'noop'`: the capture-phase dispatcher must still
 *  `stopPropagation` on it. `<Overlay>` installs its own bubble-phase Escape
 *  listener through `useDismissOnOutside`, which dismisses whenever that
 *  overlay is the top panel (`isTop`), and
 *  it is normally shadowed by the `stopPropagation` the `'dismissed'` branch
 *  does. Falling through silently would let that listener close the overlay
 *  after all, which is the double action this branch exists to prevent (caught
 *  by the fullscreen e2e). It must NOT `preventDefault`, since the UA's own
 *  fullscreen exit is the one thing that should still happen.
 *
 *  Exported for unit testing, which is why `nativeFullscreen` is a parameter
 *  with a live-read default.
 *
 *  Pseudo-fullscreen is NOT covered by rule 0 and must not be: it is painted in
 *  the normal layer and dismissed through this same stack, where LIFO already
 *  gives the right answer (it registers before the overlay, so the overlay pops
 *  first and fullscreen survives). */
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

/** Wire type for a keydown the SDK forwards out of an app iframe — must match
 *  `FORWARD_KEYDOWN_TYPE` in `packages/lucidos-sdk/src/keyboardForward.ts`.
 *  Hardcoded both sides, same convention as `lucidos:ui:confirm`. */
const FORWARDED_KEYDOWN_TYPE = 'lucidos:keydown';

type ChordLike = Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key'>;

/** Whether a bare keydown that bubbled to <body> should type-to-focus the prompt
 *  textarea (desktop convenience: start typing anywhere → land in the prompt).
 *  False when an overlay is open — a dropdown / modal / popover owns keystrokes
 *  while up, so typing must search the dropdown (its own typeahead), never
 *  leak into the prompt textarea behind the inert UI. Also false on mobile, for
 *  IME composition, when a text input already has focus, with a modifier held,
 *  or for non-printable keys. One extra carve-out: a Space press whose target is
 *  the focusable transcript region — Space is a printable key, but there it must
 *  page the transcript down (native scroll of the focused container), not insert a
 *  space into the prompt. Other printables inside the transcript still type-to-
 *  focus (start typing → compose). Pure — exported for unit testing. */
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
    && !(e.key === ' ' && isThreadTranscript(e.target));
}

/** Whether a Mac text field owns this keydown. There Ctrl with a bare letter
 *  edits text: Ctrl+K deletes to the line end, Ctrl+P moves up a line. Option
 *  with an arrow moves the caret by paragraph or word. So a shortcut on either
 *  chord fires anywhere but a field. Exported for testing, with the platform as
 *  a parameter. */
export function isMacTextEditingKey(
  e: Pick<KeyboardEvent, 'metaKey' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'key' | 'target'>,
  mac: boolean = isMac,
): boolean {
  if (!mac || e.metaKey || e.shiftKey || !isTextInput(e.target)) return false;
  const ctrlLetter = e.ctrlKey && !e.altKey && /^[a-z]$/i.test(e.key);
  const optionArrow = e.altKey && !e.ctrlKey && e.key.startsWith('Arrow');
  return ctrlLetter || optionArrow;
}

/** Whether the Escape policy would act: an overlay to close, native fullscreen
 *  to stand down for, or a `data-escape-self` field, whose blur would commit
 *  the edit its own Escape cancels. */
function escapePolicyHasWork(): boolean {
  if (overlayStack.value.length > 0 || nativeFullscreenElement() !== null) return true;
  const active = typeof document === 'undefined' ? null : document.activeElement;
  return active?.hasAttribute?.('data-escape-self') === true;
}

/** Shortcuts that take their chord only where they apply. Anywhere else the
 *  key keeps its browser default, so Mod+F opens the browser's own find.
 *  `inContent` says the keydown came from a frame in the content pane. */
const CLAIMS_ONLY_WHEN: Partial<Record<ShortcutId, (inContent: boolean) => boolean>> = {
  // Never under a modal: what it shows is not the pane behind it. An open bar
  // takes the chord from anywhere, since pressing it again closes the bar.
  findInView: (inContent) => topPanelOverlay() === null
    && (findSurface.value !== null
      || (inContent ? findAvailable('content') : focusedFindSurface() !== null)),
};

function claimsChord(id: ShortcutId, inContent: boolean): boolean {
  return CLAIMS_ONLY_WHEN[id]?.(inContent) ?? true;
}

/** Who owns a chord: a shortcut id, `'escape'` for the Escape policy, or `null`
 *  (nobody, so the key keeps its default). A registry chord on Escape (⇧Esc)
 *  yields to the policy while it has work, so one press never does two things.
 *  Shared by the host keydown, app-frame forwards and the PDF preview. */
export function classifyChord(chord: ChordLike, inContent = false): ShortcutId | 'escape' | null {
  if (chord.key === 'Escape' && escapePolicyHasWork()) return 'escape';
  const id = matchShortcut(chord);
  if (id && claimsChord(id, inContent)) return id;
  if (chord.key === 'Escape') return 'escape';
  return null;
}

/** Run a chord forwarded from an app iframe against the host registry. The
 *  forward itself is proof the frame's pane is the focused pane: the Canvas
 *  pane for an app, the thread pane for a widget on the shelf (`pane`). The frame
 *  just received the keydown — but pointer events inside the iframe
 *  never reach the host's `focusPane('content')` handler, so `focusedPane` is
 *  stale. Reconcile it BEFORE dispatching, or the three-state pane toggles read
 *  the wrong state: notably `toggleContentPane` (⌘⇧3) would "focus" the
 *  already-focused pane (a no-op) instead of CLOSING it. Exported for testing. */
export function dispatchForwardedChord(chord: ChordLike, pane: 'content' | 'thread' = 'content'): void {
  const result = classifyChord(chord, pane === 'content');
  if (result === null) return;
  // The frame's own script can post this message, so a host-only shortcut
  // (Apply, the voice call) never runs from it.
  if (result !== 'escape' && shortcutDef(result).hostOnly) return;
  focusedPane.value = pane;
  if (result === 'escape') {
    // Focus is in the iframe, so activeElement is the <iframe>. The policy
    // dismisses any open host overlay (e.g. an SDK confirm) and otherwise
    // no-ops — it never touches the focused thread.
    dispatchEscape(document.activeElement);
  } else {
    SHORTCUT_ACTIONS[result]();
  }
}

/** Run a real keydown captured INSIDE a same-origin PDF preview iframe against
 *  the host shortcut registry. The frame runs no SDK, so its keydowns never
 *  reach the host. The shell's shortcuts would silently die while focus is in
 *  the preview. The chord would also fall through to Chrome's own default (⌘⇧↵
 *  opens the page context menu). The document is same-origin, so the host
 *  listens on it directly (`bridgePreviewIframeShortcuts`) and cancels the
 *  default here. The opaque HTML artifact preview forwards over
 *  `previewFrameBridge.ts` to `dispatchForwardedChord` instead.
 *
 *  Because the keydown lives in the content pane, the focused pane IS content.
 *  Reconcile `focusedPane` before dispatching, so the pane toggles read the
 *  right state (same reason as the forwarded path).
 *  A non-shortcut chord (plain Enter on a link, normal typing) is left untouched
 *  so the preview keeps its own behavior. Returns true when it consumed the
 *  event. Exported for testing. */
export function dispatchPreviewIframeShortcut(e: KeyboardEvent): boolean {
  const route = classifyChord(e, true);
  if (route !== null && route !== 'escape') {
    e.preventDefault();
    focusedPane.value = 'content';
    SHORTCUT_ACTIONS[route]();
    return true;
  }
  if (route === 'escape') {
    // activeElement is the host <iframe> (focus is inside it) — not a text input,
    // so the policy dismisses any open host overlay and otherwise no-ops. The
    // fullscreen stand-down counts as not consumed here: the host did nothing
    // and the browser's own fullscreen exit must be left alone.
    const result = dispatchEscape(document.activeElement);
    if (result === 'dismissed' || result === 'blurred') e.preventDefault();
    return result === 'dismissed' || result === 'blurred' || result === 'self-managed';
  }
  return false;
}

export function useKeyboardShortcuts(): void {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Registry-driven shortcuts (Search, New thread, Close cascade, Zoom …).
      // Each dispatches against the user's CURRENT binding, so customizing a
      // shortcut in Settings just works. The single-key 'c'/'t' shortcuts were
      // dropped — bare letters now fall through to type-to-focus below.
      // Runs BEFORE the Tab trap so a user who rebinds a shortcut onto Tab
      // still gets it — the trap only claims Tab presses that match no binding.
      // An Escape chord was settled in the capture phase, so 'escape' is not
      // ours to act on here.
      const route = isMacTextEditingKey(e) ? null : classifyChord(e);
      if (route !== null && route !== 'escape') {
        e.preventDefault();
        SHORTCUT_ACTIONS[route]();
        return;
      }

      // Enter, while the transcript scroll region ITSELF is focused (as ⌘↑/⌘↓
      // turn-nav leaves it), toggles the expand/collapse of the highlighted
      // (nav-focus-marked) turn. Gated on `.thread-content` being the direct target
      // so a focused link/button inside a response keeps its native Enter, and the
      // prompt textarea keeps Enter-to-send. ⌘⇧Enter (maximizePaneGroup) is already
      // claimed by the registry above; only unmodified Enter reaches here. Consumes
      // the key only when a turn was actually toggled.
      if (
        e.key === 'Enter' &&
        !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey &&
        (e.target as HTMLElement | null)?.matches?.('.thread-content') &&
        toggleNavigatedTurnCollapsed()
      ) {
        e.preventDefault();
        return;
      }

      // Tab: an open overlay routes it first. A dialog contains it, and a
      // popover closes on a Tab from outside it. Then the per-pane Tab trap
      // cycles the focused pane. A handler that already consumed it wins.
      if (e.key === 'Tab') {
        if (e.defaultPrevented || handleOverlayTab(e)) return;
        if (handlePaneTab(e)) e.preventDefault();
        return;
      }

      // Auto-focus prompt on typing (desktop only, printable characters).
      // Skipped while an overlay is open so typing searches the dropdown /
      // modal instead of leaking into the prompt textarea behind it.
      // The keydown targeted <body>, so the browser won't insert the char
      // into the newly-focused textarea — insert it manually.
      if (shouldTypeToFocusPrompt(e, { mobile: isMobile(), overlayOpen: overlayStack.value.length > 0 })) {
        focusPromptNow();
        e.preventDefault();
        document.execCommand('insertText', false, e.key);
      }
    }

    function handleKeyUp(e: KeyboardEvent) {
      // Dismiss scale panel when Cmd/Ctrl is released
      if ((e.key === 'Meta' || e.key === 'Control') && scaleModalOpen.value) {
        dismissScaleModal();
      }
    }

    // Escape is dispatched in the CAPTURE phase so it runs before any element's
    // own keydown handler (e.g. the prompt textarea's blur), giving the central
    // policy first say: dismiss the top overlay, else blur, else no-op. When an
    // overlay is dismissed we stop propagation so nothing downstream double-acts.
    function handleEscapeCapture(e: KeyboardEvent) {
      if (e.key !== 'Escape') return;
      // A registry chord on Escape (⇧Esc) runs here, in capture, and stops the
      // event, so no element's own Escape handler also acts on the press.
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
        // The browser is taking this Escape to leave fullscreen, so no page
        // handler may act on it: stop it here rather than let it reach
        // `<Overlay>`'s own bubble-phase Escape (which would close the overlay
        // and make one keypress do two things). Deliberately no preventDefault,
        // which is what leaves the UA's fullscreen exit alone.
        e.stopPropagation();
      } else if (result === 'blurred') {
        e.preventDefault();
        // Escape blurred a text input (e.g. the prompt) → focus went to <body>.
        // Pull it back onto the focused pane's scroll surface so the Arrow/Page
        // keys keep scrolling that pane (the prompt → transcript case the user
        // hits after Escaping the message box). Gentle: skips a compose thread
        // with no transcript, so it never silently re-grabs the just-blurred prompt.
        reconcilePaneFocus(focusedPane.value);
      }
      // 'self-managed' / 'noop' fall through untouched: the focused element's
      // own keydown Escape handler gets the event (and may preventDefault).
    }

    // Keydowns fired inside an app iframe never reach this document, so the
    // shortcuts above silently die whenever an app has focus in the content
    // pane. The SDK forwards shortcut-shaped chords up via postMessage
    // (keyboardForward.ts); re-dispatch them against the same registry here.
    function handleForwardedKeydown(e: MessageEvent) {
      const data = e.data as {
        type?: unknown; key?: unknown;
        metaKey?: unknown; ctrlKey?: unknown; shiftKey?: unknown; altKey?: unknown;
      } | null;
      if (!data || typeof data !== 'object' || data.type !== FORWARDED_KEYDOWN_TYPE) return;
      if (typeof data.key !== 'string') return;
      // Only honor chords from a current app iframe — a nested embed/ad inside an
      // app, or an unrelated window, must not drive host shortcuts.
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
