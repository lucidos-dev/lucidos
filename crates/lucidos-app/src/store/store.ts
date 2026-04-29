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
import type { EventChannel, StoredSection } from '../generated/thread-lifecycle';
import { isMobile } from '../utils/viewport';
import { scanDraftIds, loadDraftText, loadDraftUpdatedAt, FOCUSED_DRAFT_KEY, FOCUSED_THREAD_KEY } from '../utils/draftStorage';
import { draftTitle } from '../utils/draftTitle';

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
  panelOverlay.value = null;
}
import type { Change, CCModelValue, CCReasoningEffort } from '../api/client';
import { markSwUpdateDismissed } from '../hooks/sw-update';

// --- Settings subview ---
export type SettingsSubview = 'main' | 'models' | 'appearance' | 'memory' | 'devices' | 'accounts' | 'backup' | 'repositories' | 'disk-usage';
export const settingsSubview = signal<SettingsSubview>('main');
/** Anchor to scroll/highlight after navigating from Search Everywhere. SettingsView clears it after applying. */
export const settingsScrollTarget = signal<string | null>(null);

export const SETTINGS_NAV_ITEMS: Array<{ key: Exclude<SettingsSubview, 'main'>; label: string }> = [
  { key: 'models', label: 'Models' },
  { key: 'appearance', label: 'Appearance' },
  { key: 'devices', label: 'Devices' },
  { key: 'accounts', label: 'Accounts' },
  { key: 'repositories', label: 'Repositories' },
  { key: 'backup', label: 'Backup' },
  { key: 'memory', label: 'Memory' },
  { key: 'disk-usage', label: 'Disk Usage' },
];

// --- Active menu item ---
const _savedMenuItem = localStorage.getItem('lucidos-active-menu-item');
export const activeMenuItem = signal<MenuItem>(
  _savedMenuItem && (MENU_ITEMS as readonly string[]).includes(_savedMenuItem)
    ? (_savedMenuItem as MenuItem)
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
export const DEFAULT_DRAWER_WIDTH = 300;
export const MIN_DRAWER_WIDTH = 200;
export const threadDrawerWidth = signal(
  Number(localStorage.getItem('lucidos-thread-drawer-width')) || DEFAULT_DRAWER_WIDTH
);
export const focusedThreadId = signal<string | null>(
  localStorage.getItem(FOCUSED_THREAD_KEY)
);

// True while the prompt FLIP animation is sliding from compose→thread position.
// ThreadView gates its content behind this to avoid rendering exchanges mid-slide.
export const promptAnimating = signal(false);

// True when the next focusThread should trigger slide-up reveal animation.
// Set only by handleDismissThread → focusThread (Done → next thread).
export const revealOnFocus = signal(false);

// --- Thread channel filter ---
export type ThreadChannel = EventChannel;
export const ALL_CHANNELS: ThreadChannel[] = [...EVENT_CHANNELS];

function restoreThreadChannelFilter(): Set<ThreadChannel> {
  try {
    const saved = localStorage.getItem('lucidos-thread-channel-filter');
    if (saved) {
      const parsed = JSON.parse(saved) as ThreadChannel[];
      const valid = parsed.filter(s => ALL_CHANNELS.includes(s));
      if (valid.length > 0) return new Set(valid);
    }
  } catch { /* ignore */ }
  return new Set(ALL_CHANNELS);
}

export const threadChannelFilter = signal<Set<ThreadChannel>>(restoreThreadChannelFilter());

// --- Per-trigger filter ---
// Trigger IDs the user has explicitly hidden. Empty Set = "show all triggers"
// (keeps the default exhaustive — any trigger newly created later is visible
// without needing to opt it in). Only consulted when 'trigger' is in
// threadChannelFilter; otherwise the channel filter already hides them all.
export const excludedTriggerIds = signal<Set<string>>(loadStringSet('lucidos-excluded-trigger-ids'));

/** True when the user has narrowed the drawer below "everything visible". */
export const threadFilterActive = computed(() =>
  threadChannelFilter.value.size < ALL_CHANNELS.length
  || excludedTriggerIds.value.size > 0,
);

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
  // Dismiss = user acknowledged any prior failed/aborted state. Drop the red
  // status dot immediately, before the ThreadDismissed SSE round-trip lands.
  if (dismissingThreadIds.value.has(thread.meta.id)) return 'idle';
  if (applyingNowThreadIds.value.has(thread.meta.id)) return 'running';
  // Pending user messages = request sent, thread is running before SSE event arrives
  if (thread.pendingUserMessages.length > 0) return 'running';
  return thread.meta.status;
}

/** All threads whose display section is 'review'. Threads with unsent drafts
 *  are excluded so the badge count matches what the drawer shows — drafts
 *  appear in the Drafts section, not Review. The desktop carve-out matches
 *  ThreadDrawer.categorizeThreads: a focused-on-desktop draft stays in its
 *  natural section. */
export function getReviewThreads(): ThreadState[] {
  const result: ThreadState[] = [];
  const draftMap = drafts.value;
  const focused = focusedThreadId.value;
  const mobile = isMobile();
  for (const thread of threadMap.value.values()) {
    if (thread.meta.section !== 'unread') continue;
    if (draftMap.has(thread.meta.id) && (mobile || thread.meta.id !== focused)) continue;
    const display = displaySection(
      thread.meta.section as StoredSection, effectiveThreadStatus(thread),
      thread.meta.pinned, thread.meta.activeChildrenCount > 0,
    );
    if (display === 'review') result.push(thread);
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
const savedMobileView = localStorage.getItem('lucidos-mobile-view') as MobileView | null;
export const mobileView = signal<MobileView>(
  savedMobileView && (MOBILE_VIEWS as string[]).includes(savedMobileView) ? savedMobileView : 'thread'
);
export function setMobileView(view: MobileView) {
  mobileView.value = view;
  localStorage.setItem('lucidos-mobile-view', view);
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
export const repoSelectedChangeId = signal<string | null>(null);
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
/** Thread IDs where dismiss is in progress (prevents duplicate API calls). */
export const dismissingThreadIds = signal<Set<string>>(new Set());
/** Thread IDs where CC changes discard is in progress (hides Apply, shows "Discard..."). */
export const discardingCCThreadIds = signal<Set<string>>(new Set());
/** All changes from Claude Code sessions. Updated via SSE push or API fetch. */
export const changes = signal<Change[]>([]);
/** Recently applied/reverted changes. Updated via SSE push. */
export const appliedChanges = signal<Change[]>([]);
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
export const contextExpanded = signal(
  localStorage.getItem('lucidos-context-expanded') === 'true'
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

export function showToast(message: string, type: ToastType = 'info', opts?: { key?: string; action?: ToastAction; onClick?: () => void; spinning?: boolean }) {
  const { key, action, onClick, spinning } = opts ?? {};
  // If a key is provided, update an existing toast with the same key instead of creating a new one
  if (key) {
    const existing = toasts.value.find((t) => t.key === key);
    if (existing) {
      toasts.value = toasts.value.map((t) => t.key === key ? { ...t, message, type, action, onClick, spinning } : t);
      return;
    }
  }
  const id = ++toastIdCounter;
  toasts.value = [...toasts.value, { id, message, type, key, action, onClick, spinning }];
  // Errors, warnings, and toasts with actions/onClick require manual dismissal; other types auto-close
  if (!key && !action && !onClick && type !== 'error' && type !== 'warning') {
    setTimeout(() => {
      toasts.value = toasts.value.filter((t) => t.id !== id);
    }, 5000);
  }
}

export function dismissToast(idOrKey: number | string) {
  toasts.value = toasts.value.filter((t) =>
    typeof idOrKey === 'string' ? t.key !== idOrKey : t.id !== idOrKey
  );
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
export const popupImageSrc = signal<string | null>(null);

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

/** Updated from SSE RecoveryProgress events. null = not recovering. */
export const recoveryProgress = signal<{ completed: number; total: number } | null>(null);

// --- Update available ---
export const updateAvailable = signal(false);

// --- Per-thread drafts ---

export interface DraftMeta {
  title: string;
  updatedAt: string;
}

export function newDraftId(): string {
  return `draft-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function bootstrapFocusedDraftId(): string {
  const saved = localStorage.getItem(FOCUSED_DRAFT_KEY);
  if (saved) return saved;
  const id = newDraftId();
  localStorage.setItem(FOCUSED_DRAFT_KEY, id);
  return id;
}

function bootstrapDrafts(): Map<string, DraftMeta> {
  const map = new Map<string, DraftMeta>();
  for (const id of scanDraftIds()) {
    map.set(id, {
      title: draftTitle(loadDraftText(id)),
      updatedAt: loadDraftUpdatedAt(id) ?? '',
    });
  }
  return map;
}

export const focusedDraftId = signal<string>(bootstrapFocusedDraftId());
export const drafts = signal<Map<string, DraftMeta>>(bootstrapDrafts());
