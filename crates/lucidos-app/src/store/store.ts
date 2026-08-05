import { signal, computed } from '@preact/signals';
import { hydratePinnedAppsFromStorage } from './actions/pinnedApps';
import type {
  ThreadQueueResponse,
  MenuItem,
  ConnectionStatus,
  Loadable,
  Notification,
  CredentialInfo,
  OAuthAccountInfo,
  PinnedAppEntry,
  InstalledPlugin,
  TriggerInfo,
  TriggerGroup,
  HistoricalTriggerInfo,
  App,
  MarketplaceCatalog,
  ConfirmState,
  ConfirmDetails,
  PromptState,
  ResponseEvent,
  ToastAction, ToastItem, ToastType,
  CredentialRequest,
  EmailConfirmRequest,
  PluginInstallRequest,
  PluginUninstallRequest,
} from './types';
import { MENU_ITEMS } from './types';
import type { AppUpdateRunning } from '../utils/tauri';
import type { ThreadState, ThreadStatus, Exchange } from './thread-events';
import { computeExchanges, isExcludedFromSections } from './thread-events';
import { getThreadEventsBump } from './threadActivity';
import { DEFAULT_CHAT_MODEL } from './models';
import { displaySection, EVENT_CHANNELS } from '../generated/thread-lifecycle';
import type { EventChannel, ArchiveState, DisplaySection } from '../generated/thread-lifecycle';
import { resetContentScroll } from '../hooks/useScrollMemory';
import type { Change, CodingAgentModelValue, CodingAgentReasoningEffort } from '../api/client';
import type { EnvironmentVariable, ModelInfo } from '../api/types';
import { markSwUpdateDismissed, markSwitchDismissed } from '../hooks/sw-update';

/** localStorage key holding the focused thread id across reloads. Focus is
 *  per-device, not worth round-tripping through the server. */
export const FOCUSED_THREAD_KEY = 'lucidos-focused-thread';

// --- Inline form (replaces 5 separate modal booleans) ---
export type InlineForm =
  | { type: 'credential'; editing?: string; request?: CredentialRequest }
  | { type: 'app-edit'; appId: string }
  | { type: 'new-app' }
  | { type: 'trigger'; triggerId?: string }
  | { type: 'email-confirm'; request: EmailConfirmRequest; sentAt?: string }
  | { type: 'plugin-install'; request: PluginInstallRequest }
  | { type: 'plugin-uninstall'; request: PluginUninstallRequest };

/** The email confirmation panel, before or after the send. `sentAt` (an ISO
 *  timestamp) present ⇒ the email went out and the panel is a read-only receipt;
 *  one field rather than a `sent` boolean + timestamp pair, so the two can't
 *  desync. The marker lives on the form — persisted panel state — rather than in
 *  component state, so an already-sent email can never present a Send button
 *  again after a remount (a Back/Forward walk onto the entry, or a reload). */
export type EmailConfirmForm = Extract<InlineForm, { type: 'email-confirm' }>;

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

/** User override for the diff whole-file toggle. `null` = no explicit choice, so
 *  the effective view defaults by file status (see `diffWholeFileEffective`):
 *  added files open as the whole file, everything else on the unified hunks.
 *  `true`/`false` = the user toggled the header button. Orthogonal to
 *  filePreviewSource (which still toggles source-vs-rendered within the whole-file
 *  view). Transient, reset to `null` whenever the previewed file changes (see
 *  store/effects.ts) so each new diff re-derives its default — like
 *  filePreviewEditing, NOT persisted across diffs or reloads. */
export const diffWholeFile = signal<boolean | null>(null);

/** When true, a diff's unified hunks render as two columns instead: the original
 *  on the left, the changed file on the right, aligned row for row.
 *
 *  Persisted, unlike `diffWholeFile`: this is a way of READING diffs rather than
 *  a per-file override, so it must not reset every time the previewed file
 *  changes. Honoured only where there is room for two columns and only for the
 *  raw hunks (see `fitsSideBySide` and `diffBodyKind`). */
export const diffSideBySide = signal(localStorage.getItem('lucidos-diff-side-by-side') === 'true');

/** Whether the diff on screen is wide enough for two columns, measured by
 *  `DiffView` (see `fitsSideBySide`) and published here because the CONTENT
 *  pane's header needs the same answer: it offers the Side by side toggle only
 *  where the toggle would do something, and a control that is present but inert
 *  on a phone or a collapsed pane is a lie about what the surface can do.
 *
 *  Written from the measuring component rather than derived, because only the
 *  DOM knows: the content pane is resizable, so this is not a function of the
 *  viewport. Defaults to true so the first paint does not flash the unified
 *  view before the ResizeObserver has run. */
export const diffFitsSideBySide = signal(true);

/** When true, the data-file preview shows an editable textarea instead of the
 *  rendered/source view. Reset to false whenever the previewed file changes
 *  (see store/effects.ts) so a stale draft toggle never carries to a new file.
 *  Not persisted — editing is always an explicit, in-session action. */
export const filePreviewEditing = signal(false);

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

/** The notification whose detail is being FETCHED, or null. Set only on
 *  `viewNotification`'s miss path, where neither loaded list holds the row (the
 *  cold push-tap deep link) and a round-trip stands between the tap and the
 *  panel.
 *
 *  Deliberately NOT a `panelOverlay` variant. `panelOverlay` is the panel nav
 *  stack's unit of history (`pushNavState` / `restoreState` / `overlaysEqual`),
 *  so a speculative write would leave a phantom entry that Back walks onto when
 *  the fetch fails. Keeping the pending state beside it means the overlay is
 *  still only ever written with a real notification in hand. */
export const notificationDetailPending = signal<string | null>(null);

export function closeInlineForm(): void {
  // Trigger forms reset the list scroll on close (Save/Cancel/Escape) so the
  // user lands at the top instead of the row they just edited. Other form
  // types preserve their underlying view's scroll.
  const form = activeInlineForm.value;
  if (form?.type === 'trigger') resetContentScroll('triggers');
  panelOverlay.value = null;
}

// --- Settings subview ---
export type SettingsSubview = 'main' | 'system' | 'models' | 'appearance' | 'memory' | 'devices' | 'accounts' | 'backup' | 'coding-agents' | 'locale' | 'marketplaces' | 'disk-usage' | 'permissions' | 'keyboard-shortcuts' | 'access' | 'environment-variables' | 'thread-queue' | 'debugging';
export type SettingsNavKey = Exclude<SettingsSubview, 'main'>;
export interface SettingsNavItem {
  key: SettingsNavKey;
  label: string;
}
/** Home-list grouping for the top-level Settings rows. Presentational only: a
 *  group is a heading above its rows, NOT a navigation level, so every category
 *  is still one tap from the Settings home. */
export type SettingsNavGroup = 'Assistant' | 'Workspace' | 'This device';
export interface SettingsHomeNavItem extends SettingsNavItem {
  group: SettingsNavGroup;
}
export const settingsSubview = signal<SettingsSubview>('main');
/** Anchor to scroll/highlight after navigating from Search Everywhere. SettingsView clears it after applying. */
export const settingsScrollTarget = signal<string | null>(null);

export const SETTINGS_SYSTEM_SUBPANEL_ITEMS: SettingsNavItem[] = [
  { key: 'thread-queue', label: 'Thread Queue' },
  { key: 'backup', label: 'Backup' },
  { key: 'memory', label: 'Memory' },
  { key: 'disk-usage', label: 'Disk Usage' },
  { key: 'environment-variables', label: 'Environment Variables' },
  { key: 'debugging', label: 'Debugging' },
];

// The top-level Settings categories, in home-list order. Two rules hold this
// list together, both learned the hard way (see
// docs/plans/2026-08-05-settings-information-architecture.md):
//
//  1. NO ENTRY IS PLATFORM-GATED. Every row here renders on every platform, so
//     the nav has one shape everywhere and "go to Settings → X" is true for
//     everyone. Platform gating belongs to a ROW or SECTION inside a category
//     (the iOS external-link target and the Tauri in-app browser toggle both
//     live inside Appearance & Behavior → Links that way). Gating a whole
//     CATEGORY is what
//     once hid the external-link setting from installed iOS PWAs, the only
//     platform it applies to: the row's own predicate was right and the
//     Experimental nav entry it sat behind was isTauri()-only. Pinned by
//     components/settings/__tests__/settings-nav-structure.test.ts.
//  2. A ROW IS A CATEGORY, NOT A SETTING. `Links` and `Experimental` were once
//     top-level rows holding one control each, peers of System's twelve
//     sections. A single control belongs in a section of a bigger category.
//
// Groups are contiguous and rendered as headings; they add no tap depth.
export const SETTINGS_NAV_ITEMS: SettingsHomeNavItem[] = [
  { key: 'models', label: 'Models', group: 'Assistant' },
  { key: 'permissions', label: 'Permissions', group: 'Assistant' },
  // Binary paths + registered repositories: everything a coding-agent thread
  // needs configured, in one place. Both halves used to live apart (paths under
  // System → Overview, repositories as their own top-level row).
  { key: 'coding-agents', label: 'Coding Agents', group: 'Assistant' },
  { key: 'accounts', label: 'Accounts', group: 'Workspace' },
  // Language + timezone: workspace-wide user preferences, and among the most
  // looked-for settings there are. Previously buried under System → Overview.
  { key: 'locale', label: 'Locale', group: 'Workspace' },
  { key: 'marketplaces', label: 'Marketplaces', group: 'Workspace' },
  // Reaching this engine from elsewhere: the mobile-access guide plus the
  // engine's network bind, which used to be a System subpanel that the guide
  // had to deep-link into.
  { key: 'access', label: 'Access', group: 'Workspace' },
  { key: 'devices', label: 'Devices', group: 'Workspace' },
  { key: 'system', label: 'System', group: 'Workspace' },
  // Widened, not renamed. Link routing moved in, and where a link opens is
  // behaviour rather than display, so the label says both (the shape JetBrains
  // uses for the same widened scope). The KEY stays the head noun, matching the
  // repo's own ampersand precedent: "Chat & triggers" is anchored `models:chat`.
  // Keeping `appearance` also keeps every persisted search recent, the LLM's
  // `settings_view` value and the SDK type stable across this restructure.
  { key: 'appearance', label: 'Appearance & Behavior', group: 'This device' },
  { key: 'keyboard-shortcuts', label: 'Keyboard Shortcuts', group: 'This device' },
];

export function settingsSubviewLabel(key: Exclude<SettingsSubview, 'main'>): string | undefined {
  return [...SETTINGS_NAV_ITEMS, ...SETTINGS_SYSTEM_SUBPANEL_ITEMS].find(item => item.key === key)?.label;
}

/** Where a subview key retired by the 2026-08-05 restructure now lives. The
 *  content did not disappear, it moved, so each maps to the category that
 *  absorbed it rather than to `main`.
 *
 *  A `Map`, not an object literal, because the lookup key is UNTRUSTED (it comes
 *  from persisted JSON). An object literal inherits `Object.prototype`, so
 *  `obj['constructor']` returns a truthy function and the migration would hand
 *  that back as a subview, landing on the blank panel it exists to prevent. */
const RETIRED_SETTINGS_SUBVIEWS = new Map<string, SettingsSubview>([
  // Both were one-control categories folded into Appearance & Behavior's Links
  // section. (`appearance` itself is NOT retired: it kept its key.)
  ['links', 'appearance'],
  ['experimental', 'appearance'],
  ['repositories', 'coding-agents'],
  ['mobile-access', 'access'],
  ['network-access', 'access'],
]);

/** Resolve a subview name that came from OUTSIDE this build into a renderable
 *  one: the persisted nav stack (`lucidos-nav-state`, restored across upgrades),
 *  or any other untrusted source.
 *
 *  The nav stack survives the upgrade that renames a subview, and
 *  `SettingsView.renderSubview` falls through to `null` for a key it no longer
 *  knows, so restoring the raw string lands the user on a BLANK Settings panel
 *  with no error. Retired keys map to the category that absorbed them;
 *  everything unrecognised falls back to the Settings home list, which is always
 *  renderable. */
export function migrateSettingsSubview(raw: unknown): SettingsSubview {
  if (typeof raw !== 'string') return 'main';
  if (raw === 'main') return 'main';
  const moved = aliasRetiredSettingsSubview(raw);
  const live = [...SETTINGS_NAV_ITEMS, ...SETTINGS_SYSTEM_SUBPANEL_ITEMS].some(i => i.key === moved);
  return live ? (moved as SettingsSubview) : 'main';
}

/** Map a retired subview key onto the category that absorbed it, leaving
 *  anything else untouched.
 *
 *  Split out from `migrateSettingsSubview` because the two callers want
 *  different treatment of an UNKNOWN key. Restoring the nav stack wants it
 *  collapsed onto `main` (there is no user to tell, and a blank panel is the
 *  alternative). A `NavigationRequested` from outside this build (a stored
 *  notification's deep link, an app built against an older SDK
 *  `SettingsViewTarget`, an LLM `navigate_ui` call) wants the alias applied but
 *  a genuinely unknown value REPORTED, since the caller can be told and a typo
 *  should not silently land on the Settings home. */
export function aliasRetiredSettingsSubview(raw: string): string {
  return RETIRED_SETTINGS_SUBVIEWS.get(raw) ?? raw;
}

// --- Active menu item ---
// The plugin Store moved out of the Apps section into its own top-level
// **Plugins** panel. Migrate older persisted state to land on it:
//  - the retired 'app-store' menu item → the Plugins panel;
//  - someone last viewing Apps → Store (the prior fold-in) → the Plugins panel.
// The former per-tab selection ('lucidos-plugins-tab') is retired — the panel
// now uses a single "Installed only" filter instead of Installed | Store tabs,
// so both that key and the older 'lucidos-apps-tab' are cleared.
{
  const savedMenu = localStorage.getItem('lucidos-active-menu-item');
  const legacyAppsTab = localStorage.getItem('lucidos-apps-tab');
  if (savedMenu === 'app-store' || (savedMenu === 'apps' && legacyAppsTab === 'store')) {
    localStorage.setItem('lucidos-active-menu-item', 'plugins');
  }
  localStorage.removeItem('lucidos-apps-tab');
  localStorage.removeItem('lucidos-plugins-tab');
}
const savedMenuItem = localStorage.getItem('lucidos-active-menu-item');
export const activeMenuItem = signal<MenuItem>(
  savedMenuItem && (MENU_ITEMS as readonly string[]).includes(savedMenuItem)
    ? (savedMenuItem as MenuItem)
    : 'files'
);

// --- Connection ---
export const connectionStatus = signal<ConnectionStatus>('connecting');
export const isConnected = computed(() => connectionStatus.value === 'connected');
export const workspaceName = signal<string>('');
export const workspacePath = signal<string>('');
export const engineStartedAt = signal<string | null>(null);
export const lucidosRelease = signal<string | null>(null);
export const lucidosReleaseDirty = signal<boolean>(false);
export const engineVersion = signal<string | null>(null);
export const latestEngineVersion = signal<string | null>(null);
export const latestTauriAppVersion = signal<string | null>(null);
/** Why the last packaged app-update check failed, or `null` when it succeeded
 *  (or has not run). Rendered in Settings → System so a failing check is
 *  DIAGNOSABLE instead of silent — swallowing it is what made a stranded 0.15.0
 *  install indistinguishable from an up-to-date one. */
export const appUpdateCheckError = signal<string | null>(null);
/** Live phase of a packaged app-update run, or `null` when none is in flight.
 *  Fed by the `app-update-progress` Tauri event (store/actions/app-update.ts);
 *  the engine has no part in it, so this stays null in a browser / PWA / dev
 *  client. Read by BOTH the progress toast and Settings → System so the two can
 *  never disagree about what the update is doing.
 *
 *  Typed to the IN-FLIGHT frames only: `cancelled` / `failed` end a run, and the
 *  handler clears this signal on them rather than storing them, so "a terminal
 *  frame parked as live state" is not representable and no reader has to
 *  re-narrow for it. */
export const appUpdateProgress = signal<AppUpdateRunning | null>(null);
/** True when the packaged update has passed the point of no return: the bundle
 *  is being swapped or the stack restarted, which kills the gateway under the
 *  page. The resulting connection/SSE failures would bury the narration, so —
 *  exactly as `engineRestarting` does for a restart — they are suppressed and
 *  only the update's own toast (`showWhileUnavailable`) stays on screen. */
export const appUpdateCommitted = computed(() => {
  const phase = appUpdateProgress.value?.phase;
  return phase === 'installing' || phase === 'restarting-services' || phase === 'relaunching';
});
/** Can the engine reach its own database? Mirrored from `/health`'s
 *  `database_reachable` by the connection poll; an older engine omits the field,
 *  which reads as `true`, so nothing changes for one.
 *
 *  An engine outlives its database (quitting Docker Desktop is the everyday dev
 *  case), and it keeps answering `/health` and streaming SSE while every query
 *  behind it fails. Without this signal the ~20 startup loads each reported that
 *  separately and the boot splash waited out its safety cap on a thread list that
 *  could never arrive. See ADR 0037 and `engine::db_health`. */
export const databaseReachable = signal(true);
/** True when the connected engine is a packaged desktop build. Routes the
 *  "Restart" control (LaunchAgent kickstart vs. dev rebuild script) and gates
 *  the Tauri-only half of the Settings Access page. Set from /health. */
export const enginePackaged = signal<boolean>(false);

/** False when the connected engine booted with no LLM provider configured (the
 *  UnconfiguredProvider sentinel — a packaged build's first run). Drives the
 *  first-run provider onboarding in the welcome surface. Set from /health.
 *  Defaults to `true` so onboarding never flashes before the first health probe
 *  lands; the probe corrects it to `false` only when the engine reports so. */
export const llmConfigured = signal<boolean>(true);

/** Provider backends the connected engine actually has configured
 *  (`vertex`/`anthropic`/`openai`/`openrouter`/`local`), from /health. Filters
 *  the chat model picker to providers the user has set up. `null` = don't filter
 *  (mock, or an older engine that doesn't report this) — the safe default so the
 *  picker is never empty before the first probe / under mock. */
export const configuredProviders = signal<string[] | null>(null);

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

// --- Threads ---
export const threadDrawerOpen = signal(
  localStorage.getItem('lucidos-thread-drawer-open') === 'true'
);
/** The active drawer view, picked from the single drawer view selector and
 *  persisted across reloads. Four mutually-exclusive views:
 *    - `all`       — the default sectioned list (Current/Saved/Archive).
 *    - `attention` — Current/Saved threads where the agent is stuck and the user
 *                    must act: awaiting an answer/permission or a failed turn
 *                    (see `threadNeedsAttention`).
 *    - `review`    — Current/Saved threads carrying a change ready to apply
 *                    (see `threadInReview`).
 *    - `running`   — Current/Saved threads actively working on a response
 *                    (see `threadIsRunning`).
 *    - `drafts`    — threads with an unsent draft (`draftThreads`).
 *  The selector and its badge counts are backend-derived (recomputed from the
 *  rehydrated `threadMap`), so only this *active selection* needs persisting; a
 *  single key both stores the choice and encodes the one-active invariant.
 *  Legacy `'attention'`/`'drafts'` values still restore; the retired `'none'`
 *  and any unknown value fall back to `all`. */
const ALT_VIEW_KEY = 'lucidos-alt-view';
export type DrawerView = 'all' | 'attention' | 'review' | 'running' | 'drafts';

function restoreDrawerView(): DrawerView {
  const saved = localStorage.getItem(ALT_VIEW_KEY);
  return saved === 'attention' || saved === 'review' || saved === 'running' || saved === 'drafts' ? saved : 'all';
}

export const drawerView = signal<DrawerView>(restoreDrawerView());

/** Select a drawer view (the sole mutator of `drawerView`). Persists the choice
 *  — clearing the key for `all` so a pristine state restores to the default —
 *  and switches the drawer to that view. Shared by the desktop and mobile
 *  threads headers via the drawer view selector. */
export function setDrawerView(view: DrawerView): void {
  drawerView.value = view;
  if (view === 'all') localStorage.removeItem(ALT_VIEW_KEY);
  else localStorage.setItem(ALT_VIEW_KEY, view);
}
export const DEFAULT_DRAWER_WIDTH = 300;
// Floor: the drawer header's icon row needs 216px (5 × 2.25rem buttons +
// 5 × 0.25rem gaps + 1rem padding at 16px/rem). 260 keeps the row intact
// plus enough of the centered title to stay readable.
export const MIN_DRAWER_WIDTH = 260;
export const THREAD_DRAWER_WIDTH_KEY = 'lucidos-thread-drawer-width';
// Clamp at load: widths persisted under an older, smaller minimum would
// otherwise render an overflowing header until the next drag snaps them up.
export const threadDrawerWidth = signal(
  Math.max(
    MIN_DRAWER_WIDTH,
    Number(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)) || DEFAULT_DRAWER_WIDTH,
  )
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

// One-shot ticket: PromptInput.submit() sets this on a compose→active send so the
// ThreadPane FLIP knows to defer + animate the textarea height collapse together
// with the position slide (instead of the textarea snapping short first). The FLIP
// consumes it and owns the height reset in every exit path, so it can't stick tall.
export const promptSendCollapsing = signal(false);

// True when the next focusThread should trigger slide-up reveal animation.
// Set only by handleArchiveThread → focusThread (Done → next thread).
export const revealOnFocus = signal(false);

// --- Thread channel filter ---
export type ThreadChannel = EventChannel;
export const ALL_CHANNELS: ThreadChannel[] = [...EVENT_CHANNELS];
export const CODING_AGENT_CHANNEL = 'claude_code' satisfies ThreadChannel;
export const CODING_AGENT_SOURCE_FILTER = 'coding-agent';
export type ThreadFilterSource = 'chat' | 'trigger' | typeof CODING_AGENT_SOURCE_FILTER;

export const THREAD_CHANNEL_FILTER_KEY = 'lucidos-thread-channel-filter';

function restoreThreadChannelFilter(): Set<ThreadChannel> {
  const saved = localStorage.getItem(THREAD_CHANNEL_FILTER_KEY);
  if (saved === null) return new Set(ALL_CHANNELS);
  try {
    const parsed = JSON.parse(saved) as ThreadChannel[];
    if (Array.isArray(parsed)) {
      return new Set(parsed.filter(s => ALL_CHANNELS.includes(s)));
    }
  } catch (err) {
    // Startup probe the user did not initiate, at module-load time where no
    // toast or Loadable surface exists yet. Self-recovery: falling back to "all
    // channels" shows MORE threads, never fewer, and the next filter click
    // overwrites the bad payload via `toggleChannel`. Logged rather than
    // swallowed so a corrupt filter is diagnosable instead of reading as the
    // user having reset it. Mirrors `restoreInputMode` below.
    console.warn('[store] dropping malformed thread channel filter payload', err);
  }
  return new Set(ALL_CHANNELS);
}

export const threadChannelFilter = signal<Set<ThreadChannel>>(restoreThreadChannelFilter());

export function threadChannelToFilterSource(channel: ThreadChannel): ThreadFilterSource {
  switch (channel) {
    case CODING_AGENT_CHANNEL:
      return CODING_AGENT_SOURCE_FILTER;
    case 'chat':
      return 'chat';
    case 'trigger':
      return 'trigger';
  }
}

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

// Empty set = "all repos". Non-empty = filter coding-agent threads to those
// cc_repo_ids only. Mirrors `selectedTriggerIds` exactly — the dropdown turns
// the Coding Agent parent indeterminate when this set is non-empty.
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

// Mirrors selectedRepoIds for app coding-agent threads. Apps live alongside
// repos under the Coding Agent parent in the filter dropdown; their selection
// set is independent so a user can pick "this repo OR this app".
const SELECTED_APP_IDS_KEY = 'lucidos-selected-app-ids';

function restoreSelectedAppIds(): Set<string> {
  try {
    const saved = localStorage.getItem(SELECTED_APP_IDS_KEY);
    if (!saved) return new Set();
    const parsed = JSON.parse(saved);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((x): x is string => typeof x === 'string'));
  } catch { return new Set(); }
}

export const selectedAppIds = signal<Set<string>>(restoreSelectedAppIds());

export function setSelectedAppIds(next: Set<string>): void {
  selectedAppIds.value = next;
  localStorage.setItem(SELECTED_APP_IDS_KEY, JSON.stringify([...next]));
}

// Whether the filter dropdown lists deleted trigger/repo/app options (those
// whose underlying entity is gone but threads still reference it, shown with a
// `(deleted)` / `(until <date>)` suffix). Default OFF — deleted entries are
// excluded unless the user opts in. A *selected* deleted option always stays
// visible regardless, so a filter restored from localStorage is clearable.
const INCLUDE_DELETED_FILTERS_KEY = 'lucidos-filter-include-deleted';

export const includeDeletedFilterOptions = signal<boolean>(
  localStorage.getItem(INCLUDE_DELETED_FILTERS_KEY) === 'true'
);

export function setIncludeDeletedFilterOptions(next: boolean): void {
  includeDeletedFilterOptions.value = next;
  localStorage.setItem(INCLUDE_DELETED_FILTERS_KEY, String(next));
}

// Complete set of selectable filter facets (every trigger/repo/app that has a
// thread), fetched from /api/v1/threads/filter-facets. Seeds the drawer "Show"
// dropdown so it lists ALL session-having triggers/repos/apps, not just those
// in the currently-loaded window. Refreshed on startup and after each full
// thread reload.
export const filterFacets = signal<Loadable<import('../api/threads').FilterFacets>>({ status: 'not-loaded' });

// --- Thread search ---
export const threadSearchQuery = signal('');
export const threadSearchResults = signal<Loadable<import('../api/threads').ThreadSearchResult[]>>({ status: 'not-loaded' });

// --- Event-driven thread store ---
export const threadMap = signal<Map<string, ThreadState>>(new Map());
export const threadsLoaded = signal(false);
/** The thread `focusThreadOrBootstrapResult` is currently fetching metadata for,
 *  or null. Set only on the miss path, where the thread is not in `threadMap`
 *  yet and a round-trip stands between the user's tap and anything appearing.
 *
 *  Two readers, both about that window:
 *   - The bootstrap focuses the thread OPTIMISTICALLY, so the pane moves on the
 *     tap and `ThreadView` renders its existing delay-gated skeleton instead of
 *     a dead interval. That is what a notification tap navigating to a thread
 *     outside the loaded window used to have none of.
 *   - `ThreadView` clears a `focusedThreadId` whose thread isn't in the map once
 *     `threadsLoaded` is true (stale-pointer cleanup). It must NOT do that to a
 *     thread whose metadata is still in flight, or the optimistic focus would be
 *     undone on the very next render. This signal is the exemption. */
export const bootstrappingThreadId = signal<string | null>(null);
/** Thread IDs whose title was set by a ThreadTitleGenerated event (authoritative). */
export const generatedTitleIds = new Set<string>();
/** Whether the server has more older threads to load (infinite scroll). */
export const threadHasMore = signal(true);
/** Whether a load-more request is currently in flight. */
export const threadLoadingMore = signal(false);
/** Total size of the archived pile from the backend (`archive_state='archived'`,
 *  unsaved) — refreshed by every `loadAllThreads` (startup / resume / reconnect).
 *  Drives the collapsed Archive section's count badge so it shows the true total
 *  rather than the loaded window. Plain signal (not `Loadable`) to match its
 *  sibling `threadMap` / `threadsLoaded`, which the same fetch populates; the
 *  badge falls back to the loaded count until this lands. */
export const archiveThreadCount = signal(0);
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
  // Pending user messages = request sent, thread is running before SSE event
  // arrives. An `unconfirmed` row is excluded: the safety refetch has given up
  // on it, so it is kept only to keep the text visible and there is no turn in
  // flight behind it. Counting it would pin the thread on 'running' for the
  // life of the page, keeping it out of Review and showing a Stop it cannot use.
  if (thread.pendingUserMessages.some(p => !p.unconfirmed)) return 'running';
  return thread.meta.status;
}

/** True for the cancellable statuses — a turn is in flight or paused on a
 *  user question. The two states briefly transition through each other (via
 *  UserQuestionAnswered) so almost every "mid-turn" check needs both. */
export function isMidTurn(status: ThreadStatus): boolean {
  return status === 'running' || status === 'waiting_for_user_answer';
}

/** True when CC is not producing output: future events won't resolve trailing
 *  Thinking spinners in non-current exchanges, so the renderer must clean
 *  them up itself. NOT the inverse of `isMidTurn` — `waiting_for_user_answer`
 *  belongs to BOTH sets (mid-turn cancellable, but quiescent for output).
 *  That overlap is exactly the bug surface this predicate addresses: a
 *  CodingAgentPromptSent for a queued mid-flight follow-up that CC paused
 *  before consuming has a stranded Thinking step which can never be resolved
 *  naturally — CC's resume events attach to the new UserQuestionAsked
 *  exchange via current-pointer routing, not the stranded one. */
export function isThreadQuiescent(status: ThreadStatus | undefined): boolean {
  return status === 'idle' || status === 'waiting_for_user_answer';
}

/** Whether the chat render should treat this thread as quiescent for the
 *  `exchangeStatus` stale-detector / trailing-spinner cleanup (the `threadIdle`
 *  prop lifted in `renderExchanges`). Quiescent by raw status, UNLESS an
 *  optimistic resume is in flight: the user just answered a pending question
 *  (`answeringThreadIds`) or typed a follow-up not yet ingested
 *  (`pendingUserMessages`).
 *
 *  Without this carve-out the answer→resume gap mislabels the answered turn as
 *  aborted: the backend flips the projection to `running` on
 *  `UserQuestionAnswered`, but the client's `meta.status` only advances when a
 *  per-event aggregate carrying `running` arrives — and the resume's first
 *  events can land while the snapshot still reads `waiting_for_user_answer`
 *  (quiescent). The answered question-divider then has steps + no terminal +
 *  `threadIdle`, and the stale-detector in `exchange-render.ts` flashes
 *  "Aborted ⚠" until the `running` aggregate finally lands (observed as an ~8s
 *  flash spanning the model's first post-answer LLM call). Mirrors the
 *  `?.status` → `undefined` → `false` (treat-as-active) fallback of the prior
 *  inline `isThreadQuiescent(threadMeta?.status)`. */
export function isRenderedThreadIdle(thread: ThreadState | undefined): boolean {
  if (!thread) return false;
  if (answeringThreadIds.value.has(thread.meta.id)) return false;
  if (thread.pendingUserMessages.length > 0) return false;
  return isThreadQuiescent(thread.meta.status);
}

export function getThreadDisplaySection(thread: ThreadState): DisplaySection {
  return displaySection(
    thread.meta.section as ArchiveState,
    effectiveThreadStatus(thread),
    thread.meta.saved,
    thread.meta.activeChildrenCount > 0,
    thread.meta.codingAgentProposed,
    thread.meta.attentionDescendantCount > 0,
  );
}

/** Threads in the Current drawer section, ignoring the active channel/trigger/
 *  repo/app filter. `displaySection` ignores `meta.state`, so the drawer-hidden
 *  carve-out is applied here too. This is the unfiltered section membership; the
 *  archive-next-focus picker walks the drawer's filter-aware render order
 *  (`orderedCurrentForReview`) so it only lands on a thread the user can
 *  actually see. */
export function getCurrentThreads(): ThreadState[] {
  const result: ThreadState[] = [];
  for (const thread of threadMap.value.values()) {
    if (isExcludedFromSections(thread)) continue;
    if (getThreadDisplaySection(thread) === 'current') result.push(thread);
  }
  return result;
}

/** Whether a thread needs the user's attention: it sits in the Current or Saved
 *  section AND the agent is stuck waiting on the user — a question or permission
 *  request (both surface as `waiting_for_user_answer`) or a failed turn the user
 *  must address. This is the "you must act, nothing progresses until you do"
 *  subset; a change merely *ready to apply* is `threadInReview`, a separate
 *  view. (`waiting_for_user_answer`/`failed` are mutually exclusive with
 *  `running`, so no running guard is needed here.) Composing/discarded threads
 *  never qualify. Shared by `attentionThreadCount` (the selector's
 *  needs-attention badge) and `attentionThreads` (the view), so the count and
 *  the filtered list can never disagree. */
export function threadNeedsAttention(thread: ThreadState): boolean {
  if (isExcludedFromSections(thread)) return false;
  const section = getThreadDisplaySection(thread);
  if (section !== 'current' && section !== 'saved') return false;
  const status = effectiveThreadStatus(thread);
  // `paused` is deliberately absent. An engine restart interrupted that turn,
  // and the engine either resumes it by itself (a Switch to new version, within
  // seconds) or hands the Continue button back via the boot floor sweep. Neither
  // is "nothing progresses until you act", and counting it here would flash the
  // badge on every version switch. It still floats to the top of Current with
  // its own dot via `reviewTier`.
  return status === 'waiting_for_user_answer' || status === 'failed';
}

/** Whether a thread is ready for review: it sits in the Current or Saved section
 *  AND carries a coding-agent change ready to apply (`codingAgentProposed`). A
 *  mid-turn (`running`) thread is excluded — a proposed change whose follow-up
 *  turn is still in flight is not yet *ready* to apply (its WaitingBanner shows
 *  Cancel, not Apply), mirroring `getCodingAgentWaitingInfo`'s running guard so
 *  the badge can't claim a thread whose Apply button isn't even showing.
 *  Independent of `threadNeedsAttention`: a thread that is both awaiting an
 *  answer AND carrying a proposed change legitimately surfaces in both views.
 *  Shared by `reviewThreadCount` and `reviewThreads`. */
export function threadInReview(thread: ThreadState): boolean {
  if (isExcludedFromSections(thread)) return false;
  const section = getThreadDisplaySection(thread);
  if (section !== 'current' && section !== 'saved') return false;
  if (effectiveThreadStatus(thread) === 'running') return false;
  return thread.meta.codingAgentProposed;
}

/** Count of threads where the agent is stuck waiting on the user — awaiting
 *  answer/permission or a failed turn (see `threadNeedsAttention`) — across the
 *  Current and Saved sections. Drives the selector's needs-attention badge. */
export const attentionThreadCount = computed(() => {
  let count = 0;
  for (const thread of threadMap.value.values()) {
    if (threadNeedsAttention(thread)) count++;
  }
  return count;
});

/** Count of threads carrying a change ready to apply (see `threadInReview`)
 *  across the Current and Saved sections. Drives the selector's review badge. */
export const reviewThreadCount = computed(() => {
  let count = 0;
  for (const thread of threadMap.value.values()) {
    if (threadInReview(thread)) count++;
  }
  return count;
});

/** Whether a thread is actively working: it sits in the Current or Saved section
 *  AND its effective status is `running` (the agent is producing a response — the
 *  same state the status dot labels "Running"). A `running` thread always routes
 *  to Current/Saved (see `displaySection`), so the section gate never drops one;
 *  it only keeps the excluded (composing/discarded) carve-out in lockstep with
 *  the sibling predicates. Independent of `threadNeedsAttention`/`threadInReview`
 *  — those exclude `running`, so the three views never claim the same thread.
 *  Shared by `runningThreadCount` (the selector's running badge) and
 *  `runningThreads` (the view) so the count and the filtered list can't disagree. */
export function threadIsRunning(thread: ThreadState): boolean {
  if (isExcludedFromSections(thread)) return false;
  const section = getThreadDisplaySection(thread);
  if (section !== 'current' && section !== 'saved') return false;
  return effectiveThreadStatus(thread) === 'running';
}

/** Count of threads actively working on a response (see `threadIsRunning`)
 *  across the Current and Saved sections. Drives the selector's running badge. */
export const runningThreadCount = computed(() => {
  let count = 0;
  for (const thread of threadMap.value.values()) {
    if (threadIsRunning(thread)) count++;
  }
  return count;
});

export const activeExchanges = computed<Exchange[]>(() => {
  const id = focusedThreadId.value;
  if (!id) return [];
  // Subscribe to ONLY this thread's events bump — other threads' streaming
  // doesn't fan out here. Then `threadMap.peek()` reads the current map
  // without a wide subscription. The bump fires on every event arrival for
  // this thread (including the SSE skeleton-create path), so the computed
  // catches a freshly-inserted thread as soon as its first event lands.
  getThreadEventsBump(id);
  const thread = threadMap.peek().get(id);
  if (!thread) return [];
  return computeExchanges(thread);
});

export const activeStreamingBuffer = computed(() => {
  const id = focusedThreadId.value;
  if (!id) return '';
  // Per-thread bump subscription — see `activeExchanges` above for the
  // pattern. `streamingBuffer` mutates per token, which is exactly what
  // bumpThreadEvents fires on, so the live token stream lands here.
  getThreadEventsBump(id);
  const thread = threadMap.peek().get(id);
  if (!thread) return '';
  return thread.streamingBuffer;
});

export const activeThreadIsComposing = computed(() => {
  const id = focusedThreadId.value;
  if (!id) return false;
  return threadMap.value.get(id)?.meta.state === 'composing';
});

// True when the prompt is in the centered "compose" layout: either the brand-new
// blank view (no focused thread and no exchanges) or a focused composing draft.
// Single source of truth for both ThreadPane's compose↔active FLIP and
// PromptInput's compose↔compose height animation, so they agree by construction:
// the FLIP fires only when this CHANGES; the height-anim only while it STAYS true.
export const composeViewActive = computed(() => {
  const id = focusedThreadId.value;
  const isEmpty = activeExchanges.value.length === 0;
  return (!id && isEmpty) || activeThreadIsComposing.value;
});

// --- Split layout ---
export const SPLIT_RATIO_KEY = 'lucidos-split-ratio';
export const splitRatio = signal(
  parseFloat(localStorage.getItem(SPLIT_RATIO_KEY) || '0.4')
);

/** Which desktop pane currently holds focus. Drives the two-stage pane toggles
 *  (a toggle first focuses an unfocused pane, then hides it on the next press),
 *  the header wash over the focused pane's header segment, and per-pane keyboard
 *  Tab routing. Desktop-only: mobile navigates between panes instead of focusing
 *  one. Not persisted — a fresh load focuses the chat pane, the primary work
 *  area. */
export type FocusedPane = 'drawer' | 'thread' | 'content';
export const focusedPane = signal<FocusedPane>('thread');
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
// Persisted in localStorage so the last-viewed pane survives the PWA being killed
// — reopening lands on the pane the user left (e.g. 'content'), not a forced reset
// to 'thread'. Session-scoping was the old behavior, chosen to avoid stranding the
// user on a content pane whose content didn't survive; that rationale is now
// obsolete because the content pane's actual content (open app/file/url) is
// independently restored from the localStorage nav stack (see navigation.ts
// `restoreState`), so a restored 'content' pane is never blank.
export const MOBILE_VIEW_KEY = 'lucidos-mobile-view';

export function getInitialMobileView(): MobileView {
  const saved = localStorage.getItem(MOBILE_VIEW_KEY);
  return saved && (MOBILE_VIEWS as string[]).includes(saved) ? (saved as MobileView) : 'thread';
}

export const mobileView = signal<MobileView>(getInitialMobileView());

export function setMobileView(view: MobileView) {
  mobileView.value = view;
  localStorage.setItem(MOBILE_VIEW_KEY, view);
}

// --- Input Mode ---
export type InputMode =
  | { type: 'do' }
  | { type: 'coding_agent' };

/** Remembers the last pick across page reloads via localStorage. The matching
 *  persist effect lives in `effects.ts`. `sendCompose` / `discardCompose` no
 *  longer reset this — the user explicitly asked for the choice to stick so
 *  picking Claude once means the next fresh compose stays on Claude too. */
function restoreInputMode(): InputMode {
  try {
    const raw = localStorage.getItem('lucidos-input-mode');
    if (!raw) return { type: 'do' };
    const parsed = JSON.parse(raw) as { type?: unknown };
    // Accept the legacy `'claude_code'` value persisted before the rename so a
    // reload doesn't silently drop the user's stuck coding-agent compose mode.
    if (parsed?.type === 'coding_agent' || parsed?.type === 'claude_code') {
      return { type: 'coding_agent' };
    }
    return { type: 'do' };
  } catch (err) {
    // Startup probe the user did not initiate — no toast/Loadable surface
    // exists at module-load time. Self-recovery: the next toggle click fires
    // the persist effect in effects.ts and overwrites the bad payload.
    console.warn('[store] dropping malformed lucidos-input-mode payload', err);
    return { type: 'do' };
  }
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

// --- Compose destination: coding target ---
// Discriminated union of where a coding-agent thread should run — the coding
// half of the compose destination (see store/composeDestination.ts). The
// destination picker writes here via `applyDestination`; chat.ts resolves it
// to the engine's `folder` request field; compose.ts binds the resolved values
// onto the promoted thread's `meta` (codingAgentKind / codingAgentFolder /
// repoId).
export type Scope =
  | { kind: 'lucidos' }
  | { kind: 'external'; repoId: string }
  | { kind: 'app'; appId: string };

const SCOPE_STORAGE_KEY = 'lucidos-coding-agent-last-scope';
// Pre coding-agent-rename key name — read once, migrated to SCOPE_STORAGE_KEY,
// then deleted so a long-lived PWA keeps the user's compose destination.
const LEGACY_SCOPE_STORAGE_KEY = 'lucidos-cc-last-scope';
const LEGACY_REPO_STORAGE_KEY = 'lucidos-cc-last-repo';

function parseStoredScope(raw: string | null): Scope | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === 'object') {
      if (parsed.kind === 'lucidos') return { kind: 'lucidos' };
      if (parsed.kind === 'external' && typeof parsed.repoId === 'string' && parsed.repoId) {
        return { kind: 'external', repoId: parsed.repoId };
      }
      if (parsed.kind === 'app' && typeof parsed.appId === 'string' && parsed.appId) {
        return { kind: 'app', appId: parsed.appId };
      }
    }
  } catch {
    // Corrupt value — fall through to migration / default.
  }
  return null;
}

function restoreScope(): Scope {
  const current = parseStoredScope(localStorage.getItem(SCOPE_STORAGE_KEY));
  if (current) return current;

  // One-time migration from the pre-rename `lucidos-cc-last-scope` key: read it,
  // rewrite under the new key, and delete the old one so the next reload reads
  // the new shape directly.
  if (localStorage.getItem(LEGACY_SCOPE_STORAGE_KEY) !== null) {
    const renamed = parseStoredScope(localStorage.getItem(LEGACY_SCOPE_STORAGE_KEY));
    localStorage.removeItem(LEGACY_SCOPE_STORAGE_KEY);
    if (renamed) {
      localStorage.setItem(SCOPE_STORAGE_KEY, JSON.stringify(renamed));
      return renamed;
    }
  }

  // Older one-time migration from the legacy `lucidos-cc-last-repo` string ('' meant
  // Lucidos, any other value was an external repo UUID). Migrate once at
  // startup and delete the legacy key so the next reload reads the new shape.
  const legacy = localStorage.getItem(LEGACY_REPO_STORAGE_KEY);
  if (legacy !== null) {
    localStorage.removeItem(LEGACY_REPO_STORAGE_KEY);
    if (legacy && legacy.length > 0) {
      const migrated: Scope = { kind: 'external', repoId: legacy };
      localStorage.setItem(SCOPE_STORAGE_KEY, JSON.stringify(migrated));
      return migrated;
    }
  }
  return { kind: 'lucidos' };
}

export const selectedScope = signal<Scope>(restoreScope());

/** The account DEFAULT coding agent (Claude Code | Codex) — the SEED for a fresh
 *  compose's backend chip. Seeded from the `coding_agent_default` preference by
 *  `loadPreferences`. Per-draft compose picks live in `composeSelections` and do
 *  NOT write this back (draft-only), so changing the chip on one draft never
 *  changes another draft or this default; `resolveCodingAgent` falls back here
 *  for an override-less draft. Bound onto the thread's meta at compose promotion
 *  (`sendCompose`). (See ADR 0006 for the workspace-scoped default.) */
export const selectedCodingAgent = signal<import('../api/types').CodingAgent>('claude-code');

/** Translate a Scope into the engine's `folder` request field. Lucidos →
 *  empty string (engine defaults to Lucidos when both folder and repo_id are
 *  empty). External → the repo UUID (engine's `resolve_folder_input` looks
 *  it up in the registry). App → workspace-relative path which
 *  `classify_resolved_folder` matches to the app branch. */
export function scopeToFolder(scope: Scope): string {
  switch (scope.kind) {
    case 'lucidos': return '';
    case 'external': return scope.repoId;
    case 'app': return `data/apps/${scope.appId}`;
  }
}

/** Read the repo UUID out of a Scope when one applies — used by code paths
 *  that still need to surface the bound repo (e.g. CodingAgentControlMenu's
 *  per-repo command listing). App + Lucidos return undefined; the menu
 *  falls back to its default Lucidos resolution. */
export function scopeToRepoId(scope: Scope): string | undefined {
  return scope.kind === 'external' ? scope.repoId : undefined;
}

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

/** A one-shot "scroll this line into view" request, consumed and cleared by
 *  whichever file preview renders next (see `consumeLineScrollTarget`). Written
 *  only by the navigate-to-a-line path; a manual line click never sets it.
 *
 *  Deliberately NOT derived from `selectedLines`. An effect keyed on the
 *  selection would re-scroll on every unrelated re-render that keeps the
 *  selection alive (an `artifactRevision` bump, a `repoPending` refetch),
 *  yanking a user who had scrolled away, and it would fight a shift-click that
 *  extends a selection upward. Mirrors `pluginScrollTarget`. */
export const lineScrollTarget = signal<number | null>(null);

/** Take the pending scroll target, clearing it so the same navigate can't
 *  scroll twice. Returns null when nothing is pending. */
export function consumeLineScrollTarget(): number | null {
  const target = lineScrollTarget.value;
  if (target !== null) lineScrollTarget.value = null;
  return target;
}

/** Turn a navigate's `line` / `line_end` pair into a selectable range, or null
 *  when there is nothing usable to select.
 *
 *  These arrive from outside the app (an app iframe's `lucidos.ui.navigate`, an
 *  LLM `navigate_ui`, an `<a href>` inside a previewed artifact), so anything
 *  that isn't a positive whole number is rejected rather than trusted: a
 *  fractional or negative line would index a row that doesn't exist and a
 *  non-number would render as `NaN` in the highlight comparison. An inverted
 *  range is swapped rather than dropped, since the author's intent is
 *  unambiguous. Whether the range fits INSIDE the file is not checked here: the
 *  line count isn't known until the content loads, and a range past the end
 *  simply highlights nothing (the file still opens, which is the point). */
export function normalizeLineRange(
  line: unknown,
  lineEnd?: unknown,
): { start: number; end: number } | null {
  const isLine = (v: unknown): v is number =>
    typeof v === 'number' && Number.isInteger(v) && v >= 1;
  if (!isLine(line)) return null;
  if (!isLine(lineEnd)) return { start: line, end: line };
  return lineEnd < line ? { start: lineEnd, end: line } : { start: line, end: lineEnd };
}

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

/** A file in a registered repository clone, named by an encoded `repo:` string.
 *
 *  The mode segment carries the qualifier both modes need, and they are
 *  different qualifiers, which is why this is a union rather than one shape with
 *  two optional fields: a `file` names a git revision, a `diff` names the Change
 *  whose hunks to show. Neither is meaningful in the other's mode. */
export type RepoLocator =
  | {
      repoId: string;
      mode: 'file';
      /** The git revision to read the file at (a branch, tag or sha), or
       *  undefined for the clone's current `HEAD`. */
      ref?: string;
      path: string;
    }
  | { repoId: string; mode: 'diff'; changeId?: string; path: string };

/** Encode a repo file locator for the panel overlay.
 *
 *  The qualifier is embedded in the mode segment (`file#<ref>`,
 *  `diff#<changeId>`) rather than added as a fourth colon-separated field, so it
 *  survives nav history persistence AND stays unambiguous: a git ref cannot
 *  contain a colon (`git check-ref-format`), and a path can, so the existing
 *  "everything after the third colon is the path" rule still holds. Without the
 *  embedded changeId, reloading on a diff view hits a spinner forever because
 *  repoDiff is runtime-only state.
 *
 *  Takes the parsed shape so this is the exact inverse of `parseRepoPath`:
 *  `encodeRepoPath(parseRepoPath(s)) === s` for every locator `s` that parses. */
export function encodeRepoPath(locator: RepoLocator): string {
  const qualifier = locator.mode === 'file' ? locator.ref : locator.changeId;
  const modeSeg = qualifier ? `${locator.mode}#${qualifier}` : locator.mode;
  return `repo:${locator.repoId}:${modeSeg}:${locator.path}`;
}

/** Decode a repo file path from the panel overlay. Returns null if not a repo path.
 *
 *  Four forms, all of them live:
 *
 *    repo:<repoId>:file:<path>              the clone's current HEAD
 *    repo:<repoId>:file#<ref>:<path>        that branch, tag or sha
 *    repo:<repoId>:diff#<changeId>:<path>   a Change's diff
 *    repo:<repoId>:diff:<path>              legacy, from nav histories persisted
 *                                           before the changeId was embedded;
 *                                           degrades to changeId-less but parses
 *
 *  Every segment must be non-empty: this is the single predicate that decides
 *  "is this a repo path" for `normalizeDataPath`, `ContentPane`'s routing, and
 *  `openEncodedRepoFilePreview`, and `file_path` reaches it from outside the app
 *  (an app iframe's `lucidos.ui.navigate` / `previewFile`, an LLM `navigate_ui`,
 *  an `<a href="repo:…">` inside a previewed artifact). A structurally
 *  incomplete encoding like `repo::file:x`, `repo:r1:file:` or `repo:r1:file#:x`
 *  would otherwise parse into an empty repoId (an "is a repo selected?" state
 *  that is neither null nor a real id), an empty path, or an empty ref, and open
 *  a preview that can only 404, instead of falling back to the data-path preview.
 *
 *  The qualifier is sliced at the FIRST `#`, so a ref that itself contains one
 *  (`#` is legal in a ref name, unlike `:`) survives intact. This is also what
 *  keeps a GitHub-style `#L510` line suffix on an href working: that suffix is
 *  stripped by `parseRepoFileHref` before the locator ever reaches here. */
export function parseRepoPath(encoded: string): RepoLocator | null {
  if (!encoded.startsWith('repo:')) return null;
  const [, repoId, modeSeg, ...rest] = encoded.split(':');
  const path = rest.join(':');
  if (!repoId || !path || !modeSeg) return null;

  const hash = modeSeg.indexOf('#');
  const mode = hash === -1 ? modeSeg : modeSeg.slice(0, hash);
  const qualifier = hash === -1 ? undefined : modeSeg.slice(hash + 1);
  // Present but empty (`file#:x`, `diff#:x`) is malformed, not "unqualified".
  if (qualifier === '') return null;

  if (mode === 'file') return { repoId, mode, ref: qualifier, path };
  if (mode === 'diff') return { repoId, mode, changeId: qualifier, path };
  return null;
}

/** Effective whole-file view state for the current diff preview. Resolves the
 *  `diffWholeFile` user override against a per-file default: an *added* file
 *  defaults to the whole-file (regular) view since its diff is 100% additions —
 *  rendering it as unified hunks just prefixes every line with `+`. Modified and
 *  deleted files default to the hunks. An explicit header toggle writes a boolean
 *  to `diffWholeFile`, which then wins until the previewed file changes (the reset
 *  in store/effects.ts puts it back to `null`). Derived from file status rather
 *  than stamped at open time so it stays correct after a reload, where `repoDiff`
 *  re-populates asynchronously under a nav-restored overlay. */
export const diffWholeFileEffective = computed<boolean>(() => {
  const override = diffWholeFile.value;
  if (override !== null) return override;
  const encoded = previewFile.value;
  if (!encoded) return false;
  const parsed = parseRepoPath(encoded);
  if (!parsed || parsed.mode !== 'diff') return false;
  const diff = repoDiff.value;
  if (diff.status !== 'loaded') return false;
  return diff.data.files.find(f => f.path === parsed.path)?.status === 'added';
});

// --- Claude Code ---
/** Change IDs currently being applied through conflict resolution or hardening revival.
 *  Set from MergeConflictDetected SSE events, cleared by ChangeApplied/ChangeApplyFailed. */
export const applyingChangeIds = signal<Set<string>>(new Set());
/** Thread IDs with an optimistic "Apply Now" in progress.
 *  Tracks the phase: 'requesting' (waiting for backend) → 'applying' (ChangeProposed arrived).
 *  Cleared when the apply completes, fails, or the backend takes over. */
export const applyingNowThreadIds = signal<Map<string, 'requesting' | 'applying'>>(new Map());
/** Whether an "Apply All" batch is currently running. Drives the busy state of
 *  the bulk Apply All / Discard All buttons. Set optimistically when the user
 *  clicks Apply All (and by the ApplyAllBatchStarted SSE event — so a batch
 *  started on another device disables the buttons here too), cleared by
 *  ApplyAllBatchCompleted (or an immediate HTTP error). The batch applies the
 *  first change synchronously and drives the rest in the background — including
 *  a multi-minute wait while it hardens an unhardened member — so without this
 *  the button looked dead the whole time. */
export const applyAllInProgress = signal(false);
/** Thread IDs where archive is in progress (prevents duplicate API calls). */
export const archivingThreadIds = signal<Set<string>>(new Set());
/** Thread IDs where CC changes discard is in progress (hides Apply, shows "Discard..."). */
export const discardingCCThreadIds = signal<Set<string>>(new Set());
/** Thread IDs where Cancel was clicked while an exchange is active. Disables
 *  the Cancel button (shows "Cancel...") and drives the spinner status label.
 *  Cleared when the thread leaves active status (via PromptInput effect). */
export const cancelingThreadIds = signal<Set<string>>(new Set());
/** Queued chat messages being removed optimistically, keyed by thread + event id. */
export const removingQueuedMessageIds = signal<Set<string>>(new Set());
export const queuedMessageRemovalKey = (threadId: string, messageId: string): string => `${threadId}:${messageId}`;
/** Thread IDs where a pending question was just answered and the agent's resume
 *  hasn't yet advanced the client's `meta.status` off `waiting_for_user_answer`.
 *  Read by `isRenderedThreadIdle` to suppress the answer→resume "Aborted" flash
 *  (see that function). Set in the `answerThreadQuestion` action; cleared once
 *  the real status leaves `waiting_for_user_answer` (PromptInput effect) or on
 *  answer failure (the action's 409/catch paths). */
export const answeringThreadIds = signal<Set<string>>(new Set());

/** Stamp a thread as awaiting question-answer resume (optimistic). */
export function markThreadAnswering(threadId: string): void {
  if (answeringThreadIds.value.has(threadId)) return;
  const next = new Set(answeringThreadIds.value);
  next.add(threadId);
  answeringThreadIds.value = next;
}

/** Drop the optimistic answering flag for a thread (resume confirmed or failed). */
export function clearThreadAnswering(threadId: string): void {
  if (!answeringThreadIds.value.has(threadId)) return;
  const next = new Set(answeringThreadIds.value);
  next.delete(threadId);
  answeringThreadIds.value = next;
}
/** Pending changes from Claude Code sessions. `Loadable<Change[]>` so a backend
 *  failure surfaces as `failed` instead of looking like "no changes" —
 *  consumers branch on all four states per `.claude/rules/frontend.md`. */
export const changes = signal<Loadable<Change[]>>({ status: 'not-loaded' });
/** Recently applied/reverted changes. Same Loadable shape as `changes`. */
export const appliedChanges = signal<Loadable<Change[]>>({ status: 'not-loaded' });
/** Per-id cache for changes fetched on-demand by `ChangeBody` when the id
 *  isn't in `changes` or `appliedChanges`. `loading` doubles as the dedup
 *  token; `failed` prevents refetching a 404. */
export const lazyChanges = signal<Map<string, Loadable<Change>>>(new Map());
/** Look up a change by id across all three chat-change sources. Returns the
 *  resolved `Change` only — callers that need to distinguish loading/failed
 *  should read `lazyChanges.value.get(id)` directly. */
export function findChangeById(id: string): Change | undefined {
  if (changes.value.status === 'loaded') {
    const pending = changes.value.data.find(c => c.id === id);
    if (pending) return pending;
  }
  if (appliedChanges.value.status === 'loaded') {
    const applied = appliedChanges.value.data.find(c => c.id === id);
    if (applied) return applied;
  }
  const lazy = lazyChanges.value.get(id);
  return lazy?.status === 'loaded' ? lazy.data : undefined;
}
/** All change IDs currently being applied — single source of truth combining
 *  change-level tracking (applyingChangeIds) and thread-level tracking (applyingNowThreadIds). */
export const busyChangeIds = computed(() => {
  const ids = new Set(applyingChangeIds.value);
  const threadIds = applyingNowThreadIds.value;
  if (threadIds.size > 0 && changes.value.status === 'loaded') {
    for (const c of changes.value.data) {
      if (c.thread_id && threadIds.has(c.thread_id)) ids.add(c.id);
    }
  }
  return ids;
});
/** Thread IDs that own a pending change currently being applied — the reverse
 *  of busyChangeIds, mapping change-level apply tracking (applyingChangeIds: an
 *  Apply All batch member, hardening revival, or conflict resolution) back onto
 *  its originating thread. Lets the focused thread's WaitingBanner show
 *  "Apply..." when its change is applied from the Changes panel, mirroring the
 *  in-thread Apply Now path (applyingNowThreadIds) which is tracked thread-side
 *  to begin with. */
export const applyingChangeThreadIds = computed(() => {
  const result = new Set<string>();
  const ids = applyingChangeIds.value;
  if (ids.size === 0 || changes.value.status !== 'loaded') return result;
  for (const c of changes.value.data) {
    if (c.thread_id && ids.has(c.id)) result.add(c.thread_id);
  }
  return result;
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
/** The toggle button that opened the file-search modal — passed to `<Overlay>`
 *  as the dismiss anchor (exempt from outside-pointerdown dismiss). */
export const fileSearchAnchor = signal<HTMLElement | null>(null);

// --- Search Everywhere ---
export const searchEverywhereOpen = signal(false);

/** The toggle button that opened the modal. Passed to the dismiss hook as the
 *  anchor so re-tapping the toggle closes via its own handler instead of the
 *  outside-pointerdown dismiss racing the touch toggle (which reopened it). */
export const searchEverywhereAnchor = signal<HTMLElement | null>(null);

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
/** Single source of truth for unread notifications. The bell badge is a pure
 *  projection of this set — there is NO separately-fetched unread count to
 *  drift from it (the old `unreadCount` number could, which is how the badge
 *  came to show unread when the inbox held none). Maintained by
 *  actions/notifications.ts: loaded on startup / resume / notification SSE, with
 *  optimistic removal on mark-read. Bounded — unread is naturally small and the
 *  load is capped — so a pathological backlog simply renders as "99+". */
export const unreadNotifications = signal<Loadable<Notification[]>>({ status: 'not-loaded' });
/** Bell-badge count — DERIVED from the unread set, never independently fetched.
 *  Because the count IS the set's length, the badge can never contradict the
 *  notifications themselves. */
export const unreadCount = computed(() =>
  unreadNotifications.value.status === 'loaded' ? unreadNotifications.value.data.length : 0,
);
const cachedFilter = localStorage.getItem('lucidos-notifications-filter');
export const notificationsFilter = signal<'all' | 'unread'>(
  cachedFilter === 'unread' ? 'unread' : 'all',
);
export const notificationsHasMore = signal(false);
export const notificationsLoadingMore = signal(false);
export const pageTitle = computed(() =>
  unreadCount.value > 0 ? `(${unreadCount.value}) Lucidos` : 'Lucidos'
);
// --- Credentials ---
export const credentials = signal<Loadable<CredentialInfo[]>>({ status: 'not-loaded' });

// --- Environment variables (Settings → Environment Variables) ---
export const environmentVariables = signal<Loadable<EnvironmentVariable[]>>({ status: 'not-loaded' });

// --- Chat model registry (Settings → Models; drives the Lucidos Agent picker) ---
export const chatModels = signal<Loadable<ModelInfo[]>>({ status: 'not-loaded' });

// --- OAuth Accounts ---
export const oauthAccounts = signal<Loadable<OAuthAccountInfo[]>>({ status: 'not-loaded' });

// --- Triggers ---
export const triggers = signal<Loadable<TriggerInfo[]>>({ status: 'not-loaded' });

/** Thread Queue panel state — queued + running background spawns plus the
 *  capacity policy. Refreshed on ThreadQueue* / CapacityPolicyChanged SSE. */
export const threadQueue = signal<Loadable<ThreadQueueResponse>>({ status: 'not-loaded' });

export const historicalTriggers = signal<Loadable<HistoricalTriggerInfo[]>>({ status: 'not-loaded' });

/** User-visible folders that organize triggers in the panel. Pure label —
 *  groups don't fire or schedule anything. Loaded from /trigger-groups on
 *  startup and kept live via SSE handlers in `thread-events.ts`. */
export const triggerGroups = signal<Loadable<TriggerGroup[]>>({ status: 'not-loaded' });

/** Per-device collapsed state for trigger-group sections in the panel,
 *  keyed by group_id. localStorage-backed so a collapsed Morning Routine
 *  section stays collapsed across reloads and engine restarts on this
 *  device, but doesn't sync across devices (phone and laptop independent). */
const COLLAPSED_TRIGGER_GROUPS_KEY = 'lucidos-collapsed-trigger-groups';

function restoreCollapsedTriggerGroups(): Set<string> {
  try {
    const saved = localStorage.getItem(COLLAPSED_TRIGGER_GROUPS_KEY);
    if (!saved) return new Set();
    const parsed = JSON.parse(saved);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((x): x is string => typeof x === 'string'));
  } catch { return new Set(); }
}

export const collapsedTriggerGroupIds = signal<Set<string>>(restoreCollapsedTriggerGroups());

export function toggleTriggerGroupCollapsed(groupId: string): void {
  const next = new Set(collapsedTriggerGroupIds.value);
  if (next.has(groupId)) next.delete(groupId);
  else next.add(groupId);
  collapsedTriggerGroupIds.value = next;
  localStorage.setItem(COLLAPSED_TRIGGER_GROUPS_KEY, JSON.stringify([...next]));
}

// --- Pending message (used to send a message from outside the chat module) ---
export const pendingChatMessage = signal<string | null>(null);

// --- Claude Code session version — bumped when a Claude Code session starts/resumes so components can re-fetch commands ---
export const codingAgentSessionVersion = signal(0);

// --- CC pending preferences — set from compose view before a session starts, consumed on first CC message ---
export const codingAgentPendingModel = signal<CodingAgentModelValue | null>(null);
export const codingAgentPendingReasoningEffort = signal<CodingAgentReasoningEffort | null>(null);

/** Reset pending CC preferences to defaults. Called on thread switch and after sending. */
export function resetCodingAgentPendingPreferences(): void {
  codingAgentPendingModel.value = null;
  codingAgentPendingReasoningEffort.value = null;
}

// --- Apps ---
export const appsList = signal<Loadable<App[]>>({ status: 'not-loaded' });
export const marketplaceCatalog = signal<Loadable<MarketplaceCatalog>>({ status: 'not-loaded' });
/** Installed plugins for the Plugins → Installed tab (from GET /plugins/installed
 *  — the event projection, no marketplace scan, so it works offline and still
 *  lists a plugin whose marketplace was later removed). */
export const installedPlugins = signal<Loadable<InstalledPlugin[]>>({ status: 'not-loaded' });
/** Incremented to force-refresh app UI iframes. Used as a cache-busting key in the
 *  iframe src so Preact naturally propagates the reload to ALL iframe instances
 *  (desktop + mobile). 0 = initial load (no cache-buster needed). */
export const appRefreshKey = signal(0);
/** When set, the app UI iframe renders from the named app coding-agent
 *  thread's worktree (`?thread_id=<id>` route) instead of the live workspace
 *  data — the WIP app preview. Cleared on: button re-click, navigating away
 *  from the thread or opening a different app (focusedThreadId / currentApp
 *  effect in `actions/wipPreview.ts`), and on terminal change events
 *  (ChangeApplied / ChangeDiscarded / ChangeReverted / ThreadArchived in
 *  thread-sync.ts, plus the AppUiRefreshRequested path in refreshAppUI).
 *  Apply removes the worktree as part of ff-merge, so cleanup must fire
 *  before the iframe re-renders — iframes do not raise `onError` for HTTP
 *  4xx responses, so SSE-driven cleanup is the only reliable signal. */
export const wipPreviewThreadId = signal<string | null>(null);
export const pinnedApps = signal<Loadable<PinnedAppEntry[]>>(
  hydratePinnedAppsFromStorage(),
);

/** The **Plugins** panel's All | Installed filter. One unified catalog list
 *  shows installed and available plugins the same way (status badge + Uninstall
 *  on installed rows); this toggle just narrows it. `false` (default) → All (the
 *  whole catalog, plus any installed plugin whose marketplace is gone); `true` →
 *  only installed plugins. Persisted so a reload returns to the same view; absent
 *  state defaults to All. */
const PLUGINS_INSTALLED_ONLY_KEY = 'lucidos-plugins-installed-only';
export const pluginsInstalledOnly = signal<boolean>(
  localStorage.getItem(PLUGINS_INSTALLED_ONLY_KEY) === 'true',
);
export function setPluginsInstalledOnly(next: boolean): void {
  pluginsInstalledOnly.value = next;
  localStorage.setItem(PLUGINS_INSTALLED_ONLY_KEY, String(next));
}
/** A plugin id the Plugins panel's list should scroll to and pulse-highlight
 *  once it renders — set by the update-notification deep-link (`navigate_ui`
 *  target `plugins`) so a tap lands the user on the exact plugin that has the
 *  pending update. Mirrors `settingsScrollTarget`; the `StoreTab` scroll effect
 *  consumes it once and clears it. */
export const pluginScrollTarget = signal<string | null>(null);
/** Inline content-pane search bar (the SearchIcon in the Apps / Plugins panel
 *  header). Filters the active list client-side — installed apps on Apps,
 *  installed plugins or the catalog on Plugins — mirroring the thread search.
 *  Shared because only one panel is visible at a time. */
export const appSearchOpen = signal(false);
export const appSearchQuery = signal('');


// --- Preferences ---
export const preferences = signal<Loadable<Record<string, string>>>({ status: 'not-loaded' });

// --- Confirm dialog ---
export const confirmState = signal<ConfirmState>({
  visible: false,
  message: '',
  okLabel: 'Delete',
});

// --- Prompt dialog ---
export const promptState = signal<PromptState>({
  visible: false,
  message: '',
});

// --- File preview modal ---
/** A file an app asked the host to show over it, without navigating the shell
 *  away (`lucidos.ui.previewFile`). Read-only, and deliberately NOT a
 *  `panelOverlay` variant: `panelOverlay` is the content pane's nav-history
 *  unit, and a glance at a cited file is not a destination the Back button
 *  should walk onto. Opened and closed through `store/actions/filePreviewModal`,
 *  which owns the view-state borrowing that makes the shared preview components
 *  render it. */
export interface FilePreviewModalState {
  /** Bumped per open, so a second `previewFile` that replaces a showing modal
   *  re-runs the component's per-open effects instead of looking unchanged. */
  id: number;
  /** The resolved locator: a workspace data path, or a `repo:` encoded path.
   *  Parsed by the renderer with `parseRepoPath`, exactly as `ContentPane` parses
   *  the panel's own file-preview path. */
  path: string;
  /** The line range the modal opened at, or null when the citation named none.
   *  Kept so the escalation into the Files panel carries the same lines. */
  range: { start: number; end: number } | null;
}
export const filePreviewModal = signal<FilePreviewModalState | null>(null);

// --- Toasts ---
let toastIdCounter = 0;
export const toasts = signal<ToastItem[]>([]);
/** Standard "passive status banner" duration. Keyed toasts default to
 *  sticky (see `scheduleAutoDismiss`); callers without an action button to
 *  wait on opt back in with this so every such banner shares one tunable. */
export const TOAST_AUTO_DISMISS_MS = 5_000;
/** Pending auto-dismiss timers for keyed toasts. Cleared when the same key is
 *  re-shown (window restarts) or when the toast is dismissed by other means
 *  (close button, explicit dismissToast call) — without this cleanup, the
 *  Map entry would survive until the setTimeout fires. */
const keyedDismissTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** Is the workspace unable to serve requests right now, for a reason that is
 *  already stated on screen by one authoritative toast?
 *
 *  Three ways in, and they are the same situation reached differently: every
 *  request in flight fails at once, all for one cause, and a per-request failure
 *  toast adds nothing over the status toast already up.
 *
 *    • `engineRestarting`: the engine goes down under us (Apply & Restart), with
 *      the UiBlockingOverlay covering the screen and the "Restarting engine…"
 *      status toast narrating it.
 *    • `appUpdateCommitted`: a packaged update past its point of no return
 *      restarts the launchd service, killing the gateway serving this page.
 *    • `!databaseReachable`: the engine is up and answering `/health`, but its
 *      database is not, so every query behind it fails. This one can last as long
 *      as Docker is down, which is exactly why one accurate toast beats twenty
 *      inaccurate ones.
 *
 *  Suppression is only ever legitimate WITH that authoritative toast, which opts
 *  in via `showWhileUnavailable`. Each producer above owns one. */
export function workspaceUnavailable(): boolean {
  return engineRestarting.value || appUpdateCommitted.value || !databaseReachable.value;
}

export function showToast(message: string, type: ToastType = 'info', opts?: { key?: string; action?: ToastAction; secondaryAction?: ToastAction; onClick?: () => void; spinning?: boolean; progress?: number | null; autoDismissMs?: number; dismissable?: boolean; showWhileUnavailable?: boolean; noAutofocus?: boolean }) {
  const { key, action, secondaryAction, onClick, spinning, progress, autoDismissMs, dismissable, showWhileUnavailable, noAutofocus } = opts ?? {};
  // While the workspace cannot serve requests, every in-flight request fails at
  // once (changes fetch, SSE, health poll, the ~20 startup loads) and they all
  // fail for the SAME reason. Suppress the resulting failure/info toasts,
  // including the service-worker "New version available" prompt a post-restart
  // frontend rebuild triggers, so the only thing on screen is the one status
  // toast that names the cause, which opts in via showWhileUnavailable. See
  // `workspaceUnavailable` for the three ways in. Toasts emitted once the window
  // closes (e.g. the "Engine restarted" / "Restart failed" / "Engine restart
  // timed out" toasts, each set after clearing the flag) show normally.
  if (workspaceUnavailable() && !showWhileUnavailable) return;
  // If a key is provided, update an existing toast with the same key instead of creating a new one
  if (key) {
    const existing = toasts.value.find((t) => t.key === key);
    if (existing) {
      toasts.value = toasts.value.map((t) => t.key === key ? { ...t, message, type, action, secondaryAction, onClick, spinning, progress, dismissable, noAutofocus } : t);
      scheduleAutoDismiss(key, autoDismissMs);
      return;
    }
  }
  const id = ++toastIdCounter;
  // Freeze the toast over the pane focused right now — a later focus switch must
  // not make it jump panes (drawer counts as the thread pane). The keyed-update
  // branch above deliberately does NOT touch `pane`, so an in-place update keeps
  // the toast where it first appeared.
  const pane = focusedPane.value === 'content' ? 'content' : 'thread';
  // Prepend so the newest toast renders at the top of the column-stacked
  // container and pushes existing ones down (the container is pinned to the
  // top of the viewport, so array order is top→bottom).
  toasts.value = [{ id, message, type, key, action, secondaryAction, onClick, spinning, progress, dismissable, noAutofocus, pane }, ...toasts.value];
  if (key) {
    scheduleAutoDismiss(key, autoDismissMs);
    return;
  }
  // Unkeyed: errors, warnings, and toasts with actions/onClick require manual dismissal; other types auto-close
  const ms = autoDismissMs ?? (action || onClick || type === 'error' || type === 'warning' ? undefined : TOAST_AUTO_DISMISS_MS);
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

/** Structurally remove a keyed toast WITHOUT the user-dismiss side effects — it
 *  does NOT clear a badge or record a build as user-dismissed. Use this to keep a
 *  toast in lockstep with the signal that drives it (e.g. hide the Refresh toast
 *  the moment the client is no longer stale): a signal-driven hide must never be
 *  mistaken for the user dismissing the prompt, which would wrongly suppress it
 *  for that build. `dismissToast` (the user path) layers the side effects on top. */
export function removeToast(key: string): void {
  // Only REASSIGN when that key is actually showing: a fresh array notifies
  // every subscriber even when it removed nothing, and callers that keep a toast
  // in lockstep with a signal make exactly that no-op call on their happy path
  // (retracting the "preferences aren't reaching the engine" banner runs on
  // every successful save). Timer cleanup below stays unconditional.
  if (toasts.value.some((t) => t.key === key)) {
    toasts.value = toasts.value.filter((t) => t.key !== key);
  }
  const timer = keyedDismissTimers.get(key);
  if (timer) {
    clearTimeout(timer);
    keyedDismissTimers.delete(key);
  }
}

export function dismissToast(idOrKey: number | string) {
  if (typeof idOrKey !== 'string') {
    toasts.value = toasts.value.filter((t) => t.id !== idOrKey);
    return;
  }
  removeToast(idOrKey);
  // User-dismiss side effect, per update surface: remember THIS build as
  // dismissed (keyed by build id in hooks/sw-update.ts) so the honest re-checks
  // don't re-surface the toast — a genuinely newer build still will. Dismiss ==
  // "defer to later": it does NOT clear the badge — the badge stays lit as the
  // persistent update affordance (the user updates from the reload badge). The
  // badge is re-derived from staleness/readiness by the sync/poll checks, so it
  // clears on its own once the client is current / the engine has switched.
  if (idOrKey === 'update-available') {
    markSwUpdateDismissed();
  } else if (idOrKey === NEW_VERSION_TOAST_KEY) {
    markSwitchDismissed();
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

/** Show a text-input modal. Resolves the entered string on OK, or `null` on
 *  Cancel / Esc / backdrop. Like {@link showConfirm}, a second call replaces a
 *  visible prompt (the prior resolves `null`) — never queues. */
export function showPrompt(
  message: string,
  options?: {
    title?: string;
    defaultValue?: string;
    placeholder?: string;
    okLabel?: string;
    cancelLabel?: string;
    multiline?: boolean;
  }
): Promise<string | null> {
  const prior = promptState.peek();
  if (prior.visible) prior.resolve?.(null);

  return new Promise((resolve) => {
    promptState.value = {
      visible: true,
      message,
      title: options?.title,
      defaultValue: options?.defaultValue,
      placeholder: options?.placeholder,
      okLabel: options?.okLabel,
      cancelLabel: options?.cancelLabel,
      multiline: options?.multiline,
      resolve,
    };
  });
}

// --- Step detail modal ---
// Set to a step ResponseEvent to open the per-step detail modal; null = closed.
export type StepDetailModalState = Extract<ResponseEvent, { type: 'step' }> | null;
export const stepDetailModal = signal<StepDetailModalState>(null);

// --- Image popup + message route panel state live in their own modules; re-exported
// so importers keep using `from '../store/store'`. ---
export * from './imagePopup';
export * from './messageRoutePanel';
export * from './backgroundActivity';

// --- Memory rebuild progress ---
/** Updated from SSE memory_rebuilding events. null = not rebuilding. */
export const memoryRebuildProgress = signal<{ processed: number; total: number; percent: number } | null>(null);

/** Updated from SSE BackupProgress events. null = not backing up. */
export const backupProgress = signal<{ phase: string; progress: number; total: number } | null>(null);

/** Bumped on every terminal backup SSE event (BackupCompleted AND
 *  BackupFailed). BackupSection re-fetches `/backup/status` when this changes
 *  so the health card reflects the new last-run outcome. */
export const backupStatusVersion = signal(0);

/** Updated from SSE RecoveryProgress events. null = not recovering. */
export const recoveryProgress = signal<{ completed: number; total: number } | null>(null);

// --- Update available ---
export const updateAvailable = signal(false);

// --- New engine version ready to switch onto (dev background rebuild) ---
// Set by the version-status poll (store/actions/engine-update.ts) when a newer
// engine binary is on disk. Drives the "New version available → Switch to new
// version" toast + the control-panel badge. Distinct from `updateAvailable` (the
// client-bundle refresh) and `restartRequired` (a restart-requiring change was
// applied): this is the honest "the rebuilt engine is READY to switch to" signal.
export const engineVersionReady = signal(false);

// --- New engine version currently building (dev background rebuild) ---
// Set by the version-status poll (store/actions/engine-update.ts) when the engine
// reports `build_state === 'building'` — a background rebuild kicked off by Apply
// is in progress but not yet ready to switch onto. Drives the spinning-refresh
// brand badge. Always false in packaged builds (no background build there).
// What the toast can say ABOUT that build (elapsed time, the commits it will
// bring) rides `engineBuildDetail`, which lives with the other background-activity
// feeds in `store/backgroundActivity.ts`. Both are written by the single
// `setEngineBuilding` writer in `store/actions/engine-update.ts`, so the boolean
// and the narration cannot drift apart.
export const engineBuilding = signal(false);

/** Whether a new engine version is actually READY to switch onto — the honest
 *  "ready for the switch" signal that agrees with the background-build scheme.
 *  Lives here (not in a component) so BOTH the control-panel badge/reload-glyph
 *  AND the restart progress-toast wording (chat-changes.ts `initiateEngineRestart`)
 *  derive from the SAME predicate — the toast must never disagree with the badge —
 *  without a chat-changes ↔ ControlPanel import cycle (same reasoning as
 *  NEW_VERSION_TOAST_KEY below).
 *
 *  In **dev** this is `engineVersionReady` alone: Apply is non-disruptive and
 *  kicks off a background rebuild, so a freshly-applied restart-requiring change
 *  (`restartRequired`) does NOT mean a new version exists yet — the switch only
 *  becomes available once that build finishes and the on-disk binary differs
 *  (the version-status poll flips `engineVersionReady`, see engine-update.ts).
 *  Restarting during the build window respawns the OLD binary, so keying off
 *  `restartRequired` here would falsely claim a new version.
 *
 *  In **packaged** there is no background build — a newer GitHub release is
 *  immediately installable — so `restartRequired` (set from the outdated-release
 *  check in connection.ts) IS the ready signal; `engineVersionReady` never fires
 *  there (the poll no-ops for packaged builds).
 *
 *  Note: `restartRequired` deliberately still gates the client-refresh ordering
 *  (client-update.ts holds a refresh until after the engine switch, even during
 *  the build window) — that is a different concern from this visible signal. */
export function engineNewVersionReady(): boolean {
  return engineVersionReady.value || (enginePackaged.value && restartRequired.value);
}

// Toast key for the poll-driven "New version available → Switch to new version"
// info toast (store/actions/engine-update.ts). Lives here (not in engine-update.ts)
// so `initiateEngineRestart` (chat-changes.ts) can dismiss it when a switch begins
// without a chat-changes ↔ engine-update import cycle — the switch progress toast
// then replaces it as the single version surface.
export const NEW_VERSION_TOAST_KEY = 'engine-new-version';

// Toast key for a failed thread-list refresh. Both surfacing sites (the SSE
// resync in thread-sync.ts and the resume sync in connection.ts) share it so a
// sustained outage replaces one toast instead of stacking a new one on every
// 3s SSE reconnect.
export const THREAD_LIST_REFRESH_TOAST_KEY = 'thread-list-refresh-failed';

// Toast keys for the two per-thread event fetches (thread-loading.ts). The LOAD
// one is still fanned out, one full snapshot per eagerly-loaded thread on boot
// and per failed thread on the recovery path, so an unkeyed card meant one
// permanent, undismissable toast PER THREAD for a single outage: dozens at once
// on an iOS PWA. Keyed, the whole fan-out collapses into one card whose copy
// counts the affected threads, and a landed fetch retracts it. The REFRESH one
// no longer fans out at all (a sync point marks instead, see "stale thread
// events"), but it keeps its key for the same reason: several threads can still
// be failing at once as the user moves between them. Two keys, not one: a LOAD
// failure means this device never got the thread's history, a REFRESH failure
// means it did not get the newest events, and neither card may retract the other
// while its own claim is still true.
export const THREAD_EVENTS_LOAD_TOAST_KEY = 'thread-events-load-failed';
export const THREAD_EVENTS_REFRESH_TOAST_KEY = 'thread-events-refresh-failed';

/** How many per-thread event fetches one fan-out may have in flight at once.
 *  Named for the FETCH rather than the load because both remaining fan-outs are
 *  full snapshot loads: the eager boot loads in `loadAllThreads`, and the
 *  failed-load retry in `runResumeSync`. The wake refresh and the SSE-reconnect
 *  resync used to be here too; they now refresh only the focused thread and mark
 *  the rest (see `markLoadedThreadsStale` in `store/actions/thread-loading.ts`).
 *
 *  Over HTTP/2 the browser applies no per-host connection cap, so an unbounded
 *  fan-out over a large workspace put ~85 requests a minute onto one connection,
 *  all racing the same 10s client deadline down a tunnel a wake had only just
 *  started re-establishing. The engine answers each in single-digit
 *  milliseconds, so the burst itself was what spent those deadlines. Four keeps
 *  the link saturated without the herd.
 *
 *  Deliberately PER FAN-OUT, not a global semaphore. A recovery wake can run both
 *  of the remaining ones concurrently (the failed-load retry, and `loadAllThreads`'
 *  own eager loads), so the real ceiling there is eight. A global cap would buy
 *  little and cost the property that matters most on a wake: the focused thread's
 *  fetch would have to queue behind unrelated background work.
 *
 *  Lives here rather than in `thread-loading.ts` for the same reason the toast
 *  keys above do: its other consumer is an action module that would otherwise
 *  import it across a mocked boundary. */
export const THREAD_EVENTS_FETCH_CONCURRENCY = 4;

// Toast key for the "frontend change applied — takes effect on Switch" hint,
// shown when the engine emits FrontendUpdateDeferred (a frontend-only Apply
// couldn't advance the served client in-process because an engine version
// change is pending — see engine::frontend_refresh INV-A + engine-update.ts's
// handleFrontendUpdateDeferred). Keyed so repeated frontend-only applies while
// a Switch is pending coalesce into one toast. Lives here (not engine-update.ts)
// so initiateEngineRestart (chat-changes.ts) can collapse it into the switch
// progress toast without an import cycle — same pattern as NEW_VERSION_TOAST_KEY.
export const FRONTEND_UPDATE_DEFERRED_TOAST_KEY = 'engine-frontend-update-deferred';

// Sibling of the key above for the STRANDED case (handleFrontendUpdateStranded):
// the frontend change rebuilt but the engine serves a dist/ that will never
// receive it, so no Switch will deliver it. A separate key so a stranded warning
// can't be coalesced into — or dismissed by — the "arrives on Switch" hint, which
// would be actively misleading.
export const FRONTEND_UPDATE_STRANDED_TOAST_KEY = 'engine-frontend-update-stranded';

// --- Service worker build id ---
/** BUILD_ID of the active service worker (stamped into sw.js by the
 *  `lucidos-sw-stamp` Vite plugin — see vite.config.ts), reported by the SW on
 *  request and shown in the control panel. A debugging aid for "did the new
 *  build's SW actually take over?": it's the same value whose byte-change fires
 *  the update toast, so an unchanged id across an apply means the SW never
 *  updated. `null` until the SW answers; the live dev server reports the
 *  un-stamped `__LUCIDOS_BUILD_ID__` placeholder (shown as "dev"). */
export const serviceWorkerBuildId = signal<string | null>(null);
