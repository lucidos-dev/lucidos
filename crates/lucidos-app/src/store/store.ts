import { signal, computed } from '@preact/signals';
import type {
  MenuItem,
  ConnectionStatus,
  Loadable,
  Notification,
  CredentialInfo,
  OAuthAccountInfo,
  PinnedAppEntry,
  TriggerInfo,
  HistoricalTriggerInfo,
  App,
  ConfirmState,
  ConfirmDetails,
  ToastAction, ToastItem, ToastType,
  CredentialRequest,
  EmailConfirmRequest,
} from './types';
import { MENU_ITEMS } from './types';
import type { ThreadState, ThreadStatus, Exchange } from './thread-events';
import { computeExchanges } from './thread-events';
import { DEFAULT_CHAT_MODEL } from './models';
import { displaySection, EVENT_CHANNELS } from '../generated/thread-lifecycle';
import type { EventChannel, ArchiveState, DisplaySection } from '../generated/thread-lifecycle';
import { resetContentScroll } from '../hooks/useScrollMemory';
import type { Change, CCModelValue, CCReasoningEffort } from '../api/client';
import { markSwUpdateDismissed } from '../hooks/sw-update';

/** localStorage key holding the focused thread id across reloads. Focus is
 *  per-device, not worth round-tripping through the server. */
export const FOCUSED_THREAD_KEY = 'lucidos-focused-thread';

// --- Inline form (replaces 5 separate modal booleans) ---
export type InlineForm =
  | { type: 'credential'; editing?: string; request?: CredentialRequest }
  | { type: 'app-edit'; appId: string }
  | { type: 'new-app' }
  | { type: 'trigger'; taskId?: string }
  | { type: 'email-confirm'; request: EmailConfirmRequest };

// --- Panel overlay (discriminated union replacing 6 independent signals) ---
export type PanelOverlay =
  | { type: 'form'; form: InlineForm }
  | { type: 'app-ui'; app: App }
  | { type: 'file-preview'; path: string }
  | { type: 'url-preview'; url: string }
  | { type: 'notification-detail'; notification: Notification }
  | null;

export const panelOverlay = signal<PanelOverlay>(null);

// Computed aliases for backward compatibility (read-only)
export const activeInlineForm = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'form' ? o.form : null;
});
export const currentApp = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'app-ui' ? o.app : null;
});
export const previewFile = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'file-preview' ? o.path : null;
});

/** When true, file preview shows raw source instead of rendered output (for md, html, csv, svg). */
export const filePreviewSource = signal(localStorage.getItem('lucidos-file-preview-source') === 'true');

/** CSS-based pseudo-fullscreen fallback for mobile (when native Fullscreen API is unavailable). */
export const appPseudoFullscreen = signal(false);

export const panelUrl = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'url-preview' ? o.url : null;
});
export const webviewHasHistory = computed(() => {
  return panelUrl.value != null && panelUrl.value !== webviewInitialUrl.value;
});
export const viewingNotification = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'notification-detail' ? o.notification : null;
});

export function closeInlineForm(): void {
  // Trigger forms reset the list scroll on close (Save/Cancel/Escape) so the
  // user lands at the top instead of the row they just edited. Other form
  // types preserve their underlying view's scroll.
  const form = activeInlineForm.value;
  if (form?.type === 'trigger') resetContentScroll('triggers');
  panelOverlay.value = null;
}

// --- Settings subview ---
export type SettingsSubview = 'main' | 'models' | 'appearance' | 'shortcuts' | 'memory' | 'devices' | 'accounts' | 'backup' | 'repositories' | 'disk-usage' | 'tool-permissions';
export const settingsSubview = signal<SettingsSubview>('main');
/** Anchor to scroll/highlight after navigating from Search Everywhere. SettingsView clears it after applying. */
export const settingsScrollTarget = signal<string | null>(null);

export const SETTINGS_NAV_ITEMS: Array<{ key: Exclude<SettingsSubview, 'main'>; label: string }> = [
  { key: 'models', label: 'Models' },
  { key: 'appearance', label: 'Appearance' },
  { key: 'shortcuts', label: 'Keyboard Shortcuts' },
  { key: 'devices', label: 'Devices' },
  { key: 'accounts', label: 'Accounts' },
  { key: 'repositories', label: 'Repositories' },
  { key: 'backup', label: 'Backup' },
  { key: 'memory', label: 'Memory' },
  { key: 'tool-permissions', label: 'Tool permissions' },
  { key: 'disk-usage', label: 'Disk Usage' },
];

// --- Active menu item ---
const savedMenuItem = localStorage.getItem('lucidos-active-menu-item');
export const activeMenuItem = signal<MenuItem>(
  savedMenuItem && (MENU_ITEMS as readonly string[]).includes(savedMenuItem)
    ? (savedMenuItem as MenuItem)
    : 'files'
);

// --- Connection ---
export const connectionStatus = signal<ConnectionStatus>('disconnected');
export const isConnected = computed(() => connectionStatus.value === 'connected');
export const workspaceName = signal<string>('');
export const workspacePath = signal<string>('');
export const engineStartedAt = signal<string | null>(null);
export const lucidosRelease = signal<string | null>(null);
export const lucidosReleaseDirty = signal<boolean>(false);
export const engineVersion = signal<string | null>(null);
export const latestEngineVersion = signal<string | null>(null);
export const latestTauriAppVersion = signal<string | null>(null);

// --- Model (persisted via preferences; populated by loadPreferences) ---
export const currentModel = signal(DEFAULT_CHAT_MODEL);

// --- Reasoning Effort (persisted via preferences; populated by loadPreferences) ---
export const reasoningEffort = signal('high');

// --- Animation Speed Slider (-10 to 10, 0 = normal) ---
// Stored as slider position; speedMultiplier derives the actual multiplier
export const animationSpeed = signal(
  parseInt(localStorage.getItem('lucidos-animation-speed-slider') || '0', 10) || 0
);

/** Slider position (-10..10) → speed multiplier (0.1x..10x) via 10^(v/10). */
export const speedMultiplier = computed(() => Math.pow(10, animationSpeed.value / 10));

// --- Chat ---
export const isProcessing = computed(() => {
  const status = activeThreadStatus.value;
  return status === 'running';
});

// --- Threads ---
export const threadDrawerOpen = signal(
  localStorage.getItem('lucidos-thread-drawer-open') === 'true'
);
/** Drawer-as-drafts-filter toggle. When true, the thread list collapses to a
 *  single "Drafts" section showing every thread with an unsent draft (composing
 *  threads + existing threads with composeText/composeImages). Session-only —
 *  always starts off; the user opts in. */
export const draftsViewActive = signal(false);
export const DEFAULT_DRAWER_WIDTH = 300;
export const MIN_DRAWER_WIDTH = 200;
export const threadDrawerWidth = signal(
  Number(localStorage.getItem('lucidos-thread-drawer-width')) || DEFAULT_DRAWER_WIDTH
);
export const focusedThreadId = signal<string | null>(
  localStorage.getItem(FOCUSED_THREAD_KEY)
);

/** Single setter that keeps focusedThreadId and FOCUSED_THREAD_KEY in lockstep.
 *  Every production-code mutation of focusedThreadId must go through here so
 *  the next reload resumes the same thread (especially compose drafts whose
 *  id was allocated client-side and never touches the server until a Send).
 *  Idempotent: hot-path callers (sendMessage, focusThread on the focused row)
 *  fire with the same id repeatedly; skip the synchronous storage write when
 *  nothing changed. */
export function setFocusedThread(id: string | null): void {
  if (focusedThreadId.peek() === id) return;
  focusedThreadId.value = id;
  if (id) {
    localStorage.setItem(FOCUSED_THREAD_KEY, id);
  } else {
    localStorage.removeItem(FOCUSED_THREAD_KEY);
  }
}

// True while the prompt FLIP animation is sliding from compose→thread position.
// ThreadView gates its content behind this to avoid rendering exchanges mid-slide.
export const promptAnimating = signal(false);

// True when the next focusThread should trigger slide-up reveal animation.
// Set only by handleArchiveThread → focusThread (Done → next thread).
export const revealOnFocus = signal(false);

// --- Thread channel filter ---
export type ThreadChannel = EventChannel;
export const ALL_CHANNELS: ThreadChannel[] = [...EVENT_CHANNELS];

export const THREAD_CHANNEL_FILTER_KEY = 'lucidos-thread-channel-filter';

function restoreThreadChannelFilter(): Set<ThreadChannel> {
  const saved = localStorage.getItem(THREAD_CHANNEL_FILTER_KEY);
  if (saved === null) return new Set(ALL_CHANNELS);
  try {
    const parsed = JSON.parse(saved) as ThreadChannel[];
    if (Array.isArray(parsed)) {
      return new Set(parsed.filter(s => ALL_CHANNELS.includes(s)));
    }
  } catch { /* ignore */ }
  return new Set(ALL_CHANNELS);
}

export const threadChannelFilter = signal<Set<ThreadChannel>>(restoreThreadChannelFilter());

// Empty set = "all triggers". Non-empty = filter to those trigger_ids only.
const SELECTED_TRIGGER_IDS_KEY = 'lucidos-selected-trigger-ids';

function restoreSelectedTriggerIds(): Set<string> {
  try {
    const saved = localStorage.getItem(SELECTED_TRIGGER_IDS_KEY);
    if (!saved) return new Set();
    const parsed = JSON.parse(saved);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((x): x is string => typeof x === 'string'));
  } catch { return new Set(); }
}

export const selectedTriggerIds = signal<Set<string>>(restoreSelectedTriggerIds());

export function setSelectedTriggerIds(next: Set<string>): void {
  selectedTriggerIds.value = next;
  localStorage.setItem(SELECTED_TRIGGER_IDS_KEY, JSON.stringify([...next]));
}

// Empty set = "all repos". Non-empty = filter Claude Code threads to those
// cc_repo_ids only. Mirrors `selectedTriggerIds` exactly — the dropdown turns
// the Claude Code parent indeterminate when this set is non-empty.
const SELECTED_REPO_IDS_KEY = 'lucidos-selected-repo-ids';

function restoreSelectedRepoIds(): Set<string> {
  try {
    const saved = localStorage.getItem(SELECTED_REPO_IDS_KEY);
    if (!saved) return new Set();
    const parsed = JSON.parse(saved);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((x): x is string => typeof x === 'string'));
  } catch { return new Set(); }
}

export const selectedRepoIds = signal<Set<string>>(restoreSelectedRepoIds());

export function setSelectedRepoIds(next: Set<string>): void {
  selectedRepoIds.value = next;
  localStorage.setItem(SELECTED_REPO_IDS_KEY, JSON.stringify([...next]));
}

// --- Thread search ---
export const threadSearchQuery = signal('');
export const threadSearchResults = signal<Loadable<import('../api/threads').ThreadSearchResult[]>>({ status: 'not-loaded' });

// --- Event-driven thread store ---
export const threadMap = signal<Map<string, ThreadState>>(new Map());
export const threadsLoaded = signal(false);
/** Thread IDs whose title was set by a ThreadTitleGenerated event (authoritative). */
export const generatedTitleIds = new Set<string>();
/** Whether the server has more older threads to load (infinite scroll). */
export const threadHasMore = signal(true);
/** Whether a load-more request is currently in flight. */
export const threadLoadingMore = signal(false);
/** Derive effective thread status, accounting for in-progress apply operations and pending messages. */
export function effectiveThreadStatus(thread: ThreadState): ThreadStatus {
  // Archive = user acknowledged any prior failed/aborted state. Drop the red
  // status dot immediately, before the ThreadArchived SSE round-trip lands.
  if (archivingThreadIds.value.has(thread.meta.id)) return 'idle';
  // Apply is *not* an optimistic running flip — the thread should stay in
  // Review while the backend either finishes a clean fast-path apply (status
  // stays Idle/Waiting) or wakes CC for harden / merge-conflict resolution
  // (CC's own activity events will transition status to Running). The
  // disabled "Apply..." button in WaitingBanner is the visual feedback.
  // Pending user messages = request sent, thread is running before SSE event arrives
  if (thread.pendingUserMessages.length > 0) return 'running';
  return thread.meta.status;
}

/** True for the cancellable statuses — a turn is in flight or paused on a
 *  user question. The two states briefly transition through each other (via
 *  UserQuestionAnswered) so almost every "mid-turn" check needs both. */
export function isMidTurn(status: ThreadStatus): boolean {
  return status === 'running' || status === 'waiting_for_user_answer';
}

export function getThreadDisplaySection(thread: ThreadState): DisplaySection {
  return displaySection(
    thread.meta.section as ArchiveState,
    effectiveThreadStatus(thread),
    thread.meta.saved,
    thread.meta.activeChildrenCount > 0,
    thread.meta.ccHasChanges,
  );
}

/** All threads whose display section is 'review'. Section membership is a pure
 *  function of thread state — thread-attached drafts route to their natural
 *  section per the lifecycle contract, so no draft carve-out is needed. */
export function getReviewThreads(): ThreadState[] {
  const result: ThreadState[] = [];
  for (const thread of threadMap.value.values()) {
    if (getThreadDisplaySection(thread) === 'review') result.push(thread);
  }
  return result;
}

/** Count of threads that actually display in the REVIEW section. */
export const attentionThreadCount = computed(() => getReviewThreads().length);

export const activeExchanges = computed<Exchange[]>(() => {
  const id = focusedThreadId.value;
  if (!id) return [];
  const thread = threadMap.value.get(id);
  if (!thread) return [];
  return computeExchanges(thread);
});

export const activeThreadStatus = computed(() => {
  const id = focusedThreadId.value;
  if (!id) return 'idle' as ThreadStatus;
  const thread = threadMap.value.get(id);
  if (!thread) return 'idle' as ThreadStatus;
  return effectiveThreadStatus(thread);
});

export const activeStreamingBuffer = computed(() => {
  const id = focusedThreadId.value;
  if (!id) return '';
  const thread = threadMap.value.get(id);
  if (!thread) return '';
  return thread.streamingBuffer;
});

export const activeThreadIsComposing = computed(() => {
  const id = focusedThreadId.value;
  if (!id) return false;
  return threadMap.value.get(id)?.meta.state === 'composing';
});

// --- Split layout ---
export const splitRatio = signal(
  parseFloat(localStorage.getItem('lucidos-split-ratio') || '0.4')
);
// --- Mobile view ---
// Single source of truth for mobile pane definitions.
// Adding or reordering a pane here automatically updates:
//   - MobileView type (union of pane names)
//   - MOBILE_VIEWS array (iteration order)
//   - PANE_INDEX map (name → swipe position)
//   - PANE_COUNT (total number of panes)
const PANE_DEFS = ['threads', 'thread', 'content'] as const;
export type MobileView = typeof PANE_DEFS[number];
export const MOBILE_VIEWS: MobileView[] = [...PANE_DEFS];
export const PANE_INDEX: Record<MobileView, number> =
  Object.fromEntries(PANE_DEFS.map((v, i) => [v, i])) as Record<MobileView, number>;
export const PANE_COUNT = PANE_DEFS.length;
// Session-scoped so iOS killing the PWA returns the user to the default 'thread'
// pane instead of stranding them wherever they happened to be (e.g. 'content'
// after opening an app).
export const MOBILE_VIEW_KEY = 'lucidos-mobile-view';

export function getInitialMobileView(): MobileView {
  const saved = sessionStorage.getItem(MOBILE_VIEW_KEY);
  return saved && (MOBILE_VIEWS as string[]).includes(saved) ? (saved as MobileView) : 'thread';
}

export const mobileView = signal<MobileView>(getInitialMobileView());

export function setMobileView(view: MobileView) {
  mobileView.value = view;
  sessionStorage.setItem(MOBILE_VIEW_KEY, view);
}

// --- Input Mode ---
export type InputMode =
  | { type: 'do' }
  | { type: 'claude_code' };

function restoreInputMode(): InputMode {
  try {
    const saved = localStorage.getItem('lucidos-input-mode');
    if (saved) {
      const parsed = JSON.parse(saved) as InputMode;
      if (parsed.type === 'do' || parsed.type === 'claude_code') {
        return parsed;
      }
    }
  } catch { /* ignore */ }
  // Migrate from old format
  const old = localStorage.getItem('lucidos-input-target');
  if (old === 'claude_code') return { type: 'claude_code' };
  return { type: 'do' };
}

export const inputMode = signal<InputMode>(restoreInputMode());

// --- Repositories ---
export interface Repository {
  id: string;
  name: string;
  path: string;
  description?: string;
}

export const repositories = signal<Loadable<Repository[]>>({ status: 'not-loaded' });
export const selectedRepoId = signal<string>(localStorage.getItem('lucidos-cc-last-repo') ?? '');  // '' = Lucidos (default)

// --- Repo File Explorer ---
export const repoSource = signal<string | null>(null); // null = workspace, string = repo ID
export const repoFiles = signal<Loadable<string[]>>({ status: 'not-loaded' });
export const repoExpandedFolders = signal<Set<string>>(new Set());

export interface DiffLine {
  type: 'context' | 'addition' | 'deletion';
  content: string;
}

export interface DiffHunk {
  old_start: number;
  old_count: number;
  new_start: number;
  new_count: number;
  lines: DiffLine[];
}

export interface DiffFile {
  path: string;
  status: 'modified' | 'added' | 'deleted';
  hunks: DiffHunk[];
}

export interface RepoDiff {
  files: DiffFile[];
}

export interface RepoPendingInfo {
  branch_name: string;
  files: string[];
  description: string;
  thread_id: string | null;
}

export const repoDiff = signal<Loadable<RepoDiff>>({ status: 'not-loaded' });
export const repoPending = signal<RepoPendingInfo | null>(null);
export const repoViewMode = signal<'all' | 'changes'>('all');
export const selectedLines = signal<{ start: number; end: number } | null>(null);
export const SELECTED_CHANGE_KEY = 'lucidos-repo-selected-change-id';
// Hydrated from localStorage so the persistence effect's first synchronous
// fire doesn't wipe a saved ID before useStartup can call restore on it.
export const repoSelectedChangeId = signal<string | null>(localStorage.getItem(SELECTED_CHANGE_KEY));
export const repoChanges = signal<Loadable<import('../api/client').RepoChangesState>>({ status: 'not-loaded' });
export const repoChangesLoadingMore = signal(false);
export const selectedChange = computed(() => {
  const id = repoSelectedChangeId.value;
  if (!id) return undefined;
  const changes = repoChanges.value;
  if (changes.status !== 'loaded') return undefined;
  return changes.data.pending.find(c => c.id === id)
    ?? changes.data.applied.find(c => c.id === id);
});

/** Encode a repo file path for the panel overlay. The changeId is embedded
 *  in the mode segment for diff mode (`diff#<changeId>`) so it survives nav
 *  history persistence — without it, reloading on a diff view hits a spinner
 *  forever because repoDiff is runtime-only state. */
export function encodeRepoPath(repoId: string, mode: 'file' | 'diff', path: string, changeId?: string): string {
  const modeSeg = mode === 'diff' && changeId ? `diff#${changeId}` : mode;
  return `repo:${repoId}:${modeSeg}:${path}`;
}

/** Decode a repo file path from the panel overlay. Returns null if not a repo path.
 *  Accepts both new (`diff#<changeId>`) and legacy (`diff`) encodings — legacy
 *  entries from older nav histories degrade to changeId-less but still parse. */
export function parseRepoPath(encoded: string): { repoId: string; mode: 'file' | 'diff'; changeId?: string; path: string } | null {
  if (!encoded.startsWith('repo:')) return null;
  const [, repoId, modeSeg, ...rest] = encoded.split(':');
  if (modeSeg === 'file') {
    return { repoId, mode: 'file', path: rest.join(':') };
  }
  if (modeSeg === 'diff') {
    return { repoId, mode: 'diff', path: rest.join(':') };
  }
  const DIFF_PREFIX = 'diff#';
  if (modeSeg?.startsWith(DIFF_PREFIX)) {
    return { repoId, mode: 'diff', changeId: modeSeg.slice(DIFF_PREFIX.length), path: rest.join(':') };
  }
  return null;
}

// --- Claude Code ---
/** Change IDs currently being applied through conflict resolution or hardening revival.
 *  Set from MergeConflictDetected SSE events, cleared by ChangeApplied/ChangeApplyFailed. */
export const applyingChangeIds = signal<Set<string>>(new Set());
/** Thread IDs with an optimistic "Apply Now" in progress.
 *  Tracks the phase: 'requesting' (waiting for backend) → 'applying' (ChangeProposed arrived).
 *  Cleared when the apply completes, fails, or the backend takes over. */
export const applyingNowThreadIds = signal<Map<string, 'requesting' | 'applying'>>(new Map());
/** Thread IDs where archive is in progress (prevents duplicate API calls). */
export const archivingThreadIds = signal<Set<string>>(new Set());
/** Thread IDs where CC changes discard is in progress (hides Apply, shows "Discard..."). */
export const discardingCCThreadIds = signal<Set<string>>(new Set());
/** Thread IDs where Cancel was clicked while an exchange is active. Disables
 *  the Cancel button (shows "Cancel...") and drives the spinner status label.
 *  Cleared when the thread leaves active status (via PromptInput effect). */
export const cancelingThreadIds = signal<Set<string>>(new Set());
/** All changes from Claude Code sessions. Updated via SSE push or API fetch. */
export const changes = signal<Change[]>([]);
/** Recently applied/reverted changes. Updated via SSE push. */
export const appliedChanges = signal<Change[]>([]);
/** Per-id cache for changes fetched on-demand by `ChangeBody` when the id
 *  isn't in `changes` or `appliedChanges`. `loading` doubles as the dedup
 *  token; `failed` prevents refetching a 404. */
export const lazyChanges = signal<Map<string, Loadable<Change>>>(new Map());
/** Look up a change by id across all three chat-change sources. Returns the
 *  resolved `Change` only — callers that need to distinguish loading/failed
 *  should read `lazyChanges.value.get(id)` directly. */
export function findChangeById(id: string): Change | undefined {
  const pending = changes.value.find(c => c.id === id);
  if (pending) return pending;
  const applied = appliedChanges.value.find(c => c.id === id);
  if (applied) return applied;
  const lazy = lazyChanges.value.get(id);
  return lazy?.status === 'loaded' ? lazy.data : undefined;
}
/** All change IDs currently being applied — single source of truth combining
 *  change-level tracking (applyingChangeIds) and thread-level tracking (applyingNowThreadIds). */
export const busyChangeIds = computed(() => {
  const ids = new Set(applyingChangeIds.value);
  const threadIds = applyingNowThreadIds.value;
  if (threadIds.size > 0) {
    for (const c of changes.value) {
      if (c.thread_id && threadIds.has(c.thread_id)) ids.add(c.id);
    }
  }
  return ids;
});
/** Whether more applied changes are available for pagination. */
export const changesHasMore = signal(false);
/** Whether we're currently loading more applied changes. */
export const changesLoadingMore = signal(false);
/** Whether the engine needs a restart (Rust changes applied). */
export const restartRequired = signal(false);
/** Whether the engine is currently restarting (blocks all user interaction). */
export const engineRestarting = signal(false);
/** A thread's contribution to the pending engine restart: its title and the
 *  commit subjects that were merged. Rendered grouped in the restart toast. */
export interface RestartGroup {
  threadId: string;
  threadTitle: string;
  commits: string[];
}

/** Applied changes requiring engine restart, grouped by originating thread. */
export const restartGroups = signal<RestartGroup[]>([]);
export const stepsExpanded = signal(
  localStorage.getItem('lucidos-steps-expanded') === 'true'
);
export const detailsExpanded = signal(
  localStorage.getItem('lucidos-details-expanded') === 'true'
);

/** Signal + toggle pair backed by a localStorage-persisted Set of "threadId:userSeq" keys. */
function createCollapsedStore(storageKey: string) {
  const sig = signal<Set<string>>(loadStringSet(storageKey));
  function toggle(threadId: string, userSeq: number): void {
    const key = `${threadId}:${userSeq}`;
    const next = new Set(sig.value);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    sig.value = next;
  }
  return [sig, toggle] as const;
}

function loadStringSet(storageKey: string): Set<string> {
  try {
    const saved = localStorage.getItem(storageKey);
    return saved ? new Set(JSON.parse(saved) as string[]) : new Set();
  } catch {
    return new Set();
  }
}

export const [collapsedExchanges, toggleExchangeCollapsed] =
  createCollapsedStore('lucidos-collapsed-exchanges');
export const [collapsedInitiators, toggleInitiatorCollapsed] =
  createCollapsedStore('lucidos-collapsed-initiators');

// --- Artifacts ---
export const artifacts = signal<Loadable<string[]>>({ status: 'not-loaded' });
export const artifactRevision = signal(0);
export const panelTitle = signal<string | null>(null);
/** The URL that was set when the browser panel was opened (via openUrl or nav restore).
 *  Used to detect whether the user has navigated within the webview. */
export const webviewInitialUrl = signal<string | null>(null);
export const fileSearchOpen = signal(false);

// --- Search Everywhere ---
export const searchEverywhereOpen = signal(false);

export const expandedFolders = signal<Set<string>>(
  (() => {
    try {
      const saved = localStorage.getItem('lucidos-expanded-folders');
      return saved ? new Set(JSON.parse(saved) as string[]) : new Set<string>();
    } catch {
      return new Set<string>();
    }
  })()
);

// --- Notifications ---
export const notifications = signal<Loadable<Notification[]>>({ status: 'not-loaded' });
export const unreadCount = signal(parseInt(localStorage.getItem('lucidos-unread-count') || '0', 10) || 0);
const cachedFilter = localStorage.getItem('lucidos-notifications-filter');
export const notificationsFilter = signal<'all' | 'unread'>(
  cachedFilter === 'unread' ? 'unread' : 'all',
);
export const notificationsHasMore = signal(false);
export const notificationsLoadingMore = signal(false);
export const notificationsModalOpen = signal(false);
export const notificationModalDetail = signal<Notification | null>(null);
export const pageTitle = computed(() =>
  unreadCount.value > 0 ? `(${unreadCount.value}) Lucidos` : 'Lucidos'
);
// --- Credentials ---
export const credentials = signal<Loadable<CredentialInfo[]>>({ status: 'not-loaded' });

// --- OAuth Accounts ---
export const oauthAccounts = signal<Loadable<OAuthAccountInfo[]>>({ status: 'not-loaded' });

// --- Triggers ---
export const triggers = signal<Loadable<TriggerInfo[]>>({ status: 'not-loaded' });

export const historicalTriggers = signal<Loadable<HistoricalTriggerInfo[]>>({ status: 'not-loaded' });

// --- Pending message (used to send a message from outside the chat module) ---
export const pendingChatMessage = signal<string | null>(null);

// --- CC session version — bumped when a CC session starts/resumes so components can re-fetch commands ---
export const ccSessionVersion = signal(0);

// --- CC pending preferences — set from compose view before a session starts, consumed on first CC message ---
export const ccPendingModel = signal<CCModelValue | null>(null);
export const ccPendingReasoningEffort = signal<CCReasoningEffort | null>(null);

/** Reset pending CC preferences to defaults. Called on thread switch and after sending. */
export function resetCCPendingPreferences(): void {
  ccPendingModel.value = null;
  ccPendingReasoningEffort.value = null;
}

// --- Apps ---
export const appsList = signal<Loadable<App[]>>({ status: 'not-loaded' });
/** When set, the app UI iframe renders at this historical commit. null = live/latest. */
export const appCommit = signal<string | null>(null);
/** Incremented to force-refresh app UI iframes. Used as a cache-busting key in the
 *  iframe src so Preact naturally propagates the reload to ALL iframe instances
 *  (desktop + mobile). 0 = initial load (no cache-buster needed). */
export const appRefreshKey = signal(0);
export const pinnedApps = signal<PinnedAppEntry[]>(
  (() => {
    try {
      const saved = localStorage.getItem('pinned_apps');
      return saved ? JSON.parse(saved) as PinnedAppEntry[] : [];
    } catch {
      return [];
    }
  })()
);


// --- Preferences ---
export const preferences = signal<Loadable<Record<string, string>>>({ status: 'not-loaded' });

// --- Confirm dialog ---
export const confirmState = signal<ConfirmState>({
  visible: false,
  message: '',
  okLabel: 'Delete',
});

// --- Toasts ---
let toastIdCounter = 0;
export const toasts = signal<ToastItem[]>([]);
/** Pending auto-dismiss timers for keyed toasts. Cleared when the same key is
 *  re-shown (window restarts) or when the toast is dismissed by other means
 *  (close button, explicit dismissToast call) — without this cleanup, the
 *  Map entry would survive until the setTimeout fires. */
const keyedDismissTimers = new Map<string, ReturnType<typeof setTimeout>>();

export function showToast(message: string, type: ToastType = 'info', opts?: { key?: string; action?: ToastAction; onClick?: () => void; spinning?: boolean; autoDismissMs?: number; dismissable?: boolean }) {
  const { key, action, onClick, spinning, autoDismissMs, dismissable } = opts ?? {};
  // If a key is provided, update an existing toast with the same key instead of creating a new one
  if (key) {
    const existing = toasts.value.find((t) => t.key === key);
    if (existing) {
      toasts.value = toasts.value.map((t) => t.key === key ? { ...t, message, type, action, onClick, spinning, dismissable } : t);
      scheduleAutoDismiss(key, autoDismissMs);
      return;
    }
  }
  const id = ++toastIdCounter;
  toasts.value = [...toasts.value, { id, message, type, key, action, onClick, spinning, dismissable }];
  if (key) {
    scheduleAutoDismiss(key, autoDismissMs);
    return;
  }
  // Unkeyed: errors, warnings, and toasts with actions/onClick require manual dismissal; other types auto-close
  const ms = autoDismissMs ?? (action || onClick || type === 'error' || type === 'warning' ? undefined : 5000);
  if (ms !== undefined) {
    setTimeout(() => {
      toasts.value = toasts.value.filter((t) => t.id !== id);
    }, ms);
  }
}

function scheduleAutoDismiss(key: string, autoDismissMs: number | undefined): void {
  const prior = keyedDismissTimers.get(key);
  if (prior) clearTimeout(prior);
  if (autoDismissMs === undefined) {
    keyedDismissTimers.delete(key);
    return;
  }
  keyedDismissTimers.set(key, setTimeout(() => dismissToast(key), autoDismissMs));
}

export function dismissToast(idOrKey: number | string) {
  toasts.value = toasts.value.filter((t) =>
    typeof idOrKey === 'string' ? t.key !== idOrKey : t.id !== idOrKey
  );
  if (typeof idOrKey === 'string') {
    const t = keyedDismissTimers.get(idOrKey);
    if (t) {
      clearTimeout(t);
      keyedDismissTimers.delete(idOrKey);
    }
  }
  if (idOrKey === 'update-available') {
    updateAvailable.value = false;
    markSwUpdateDismissed();
  }
}

export function showConfirm(
  message: string,
  okLabel = 'Delete',
  options?: {
    title?: string;
    cancelLabel?: string;
    extraAction?: ToastAction;
    variant?: 'danger' | 'default';
    details?: ConfirmDetails;
  }
): Promise<boolean> {
  // If a confirm is already visible, resolve its Promise as `false` before
  // showing the new one — second call replaces, never queues.
  const prior = confirmState.peek();
  if (prior.visible) prior.resolve?.(false);

  return new Promise((resolve) => {
    confirmState.value = {
      visible: true,
      message,
      okLabel,
      title: options?.title,
      cancelLabel: options?.cancelLabel,
      variant: options?.variant,
      resolve,
      extraAction: options?.extraAction,
      details: options?.details,
    };
  });
}

// --- Image popup ---
export interface ImagePopupState {
  images: string[];
  index: number;
}

export const popupImage = signal<ImagePopupState | null>(null);

export function openImagePopup(src: string): void {
  popupImage.value = { images: [src], index: 0 };
}

/** Open the popup with prev/next nav across every sibling thumbnail in the
 *  same `.thread-content` as `clicked`. Degrades to single-image when no
 *  siblings can be collected. */
export function openImagePopupFromThread(src: string, clicked: Element | EventTarget | null): void {
  const container = (clicked instanceof Element) ? clicked.closest('.thread-content') : null;
  if (!container) { openImagePopup(src); return; }
  const els = container.querySelectorAll<HTMLImageElement>('.image-thumbnail, .user-image-thumb');
  const seen = new Set<string>();
  const images: string[] = [];
  els.forEach(el => {
    const url = el.dataset.fullSrc || el.src;
    if (url && !seen.has(url)) { seen.add(url); images.push(url); }
  });
  const index = images.indexOf(src);
  if (index === -1) { openImagePopup(src); return; }
  popupImage.value = { images, index };
}

// --- Message route panel (anchored popover for the route badge) ---
type MessageRoutePanelSection = 'origin' | 'executor';
export interface MessageRoutePanelState {
  anchor: HTMLElement;
  exchange: Exchange;
  threadId: string;
  section: MessageRoutePanelSection;
  /** Carried forward from prior exchanges so the panel can show model/effort
   *  before the current exchange's ResponseGenerated event arrives. */
  priorModel?: string;
  priorEffort?: string;
}
export const messageRoutePanel = signal<MessageRoutePanelState | null>(null);
/** Click semantics for the route badge: opens the panel for the given exchange +
 *  section, or closes it when re-clicking the same combination. Switching to a
 *  different exchange/section opens the panel with the new contents.
 *
 *  Identity is `(threadId, userSeq, section)` rather than the anchor DOM ref
 *  because the badge button can be re-rendered between clicks (streaming
 *  updates), which would replace the DOM node and silently defeat ref equality. */
export function toggleMessageRoutePanel(state: MessageRoutePanelState): void {
  const current = messageRoutePanel.value;
  if (
    current &&
    current.threadId === state.threadId &&
    current.exchange.userSeq === state.exchange.userSeq &&
    current.section === state.section
  ) {
    messageRoutePanel.value = null;
    return;
  }
  messageRoutePanel.value = state;
}
export function closeMessageRoutePanel(): void {
  messageRoutePanel.value = null;
}

// --- Memory rebuild progress ---
/** Updated from SSE memory_rebuilding events. null = not rebuilding. */
export const memoryRebuildProgress = signal<{ processed: number; total: number; percent: number } | null>(null);

/** Updated from SSE BackupProgress events. null = not backing up/restoring. */
export const backupProgress = signal<{ phase: string; progress: number; total: number } | null>(null);

/** Bumped on every BackupCompleted SSE event. BackupSection re-fetches the
 *  list when this changes, so any mounted instance (in either layout copy)
 *  sees the new entry without each running its own completion handler. */
export const backupListVersion = signal(0);

/** Updated from SSE RecoveryProgress events. null = not recovering. */
export const recoveryProgress = signal<{ completed: number; total: number } | null>(null);

// --- Update available ---
export const updateAvailable = signal(false);

