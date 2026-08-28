import { signal, computed } from '@preact/signals';
import { hydratePinnedAppsFromStorage } from './actions/pinnedApps';
import { minDrawerWidth } from './paneMinimums';
import type {
  ThreadQueueResponse,
  MenuItem,
  ConnectionStatus,
  Loadable,
  Notification,
  CredentialInfo,
  KnownOAuthProviders,
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
  ProgressDialogState,
  ContextCapture,
  PromptState,
  ResponseEvent,
  ToastAction, ToastItem, ToastType,
  CredentialRequest,
  EmailConfirmRequest,
  PluginInstallRequest,
  PluginInstallReceipt,
  PluginUninstallRequest,
  PluginUninstallReceipt,
  IngressReading,
} from './types';
import { MENU_ITEMS } from './types';
import type { AppUpdateRunning } from '../utils/tauri';
import { cancelAppUpdate } from '../utils/tauri';
import { documentTitle } from '../utils/windowTitle';
import { restartDialogState, appUpdateDialogState } from './progressDialogCopy';
import type { EventSubscription, ThreadState, ThreadStatus, Exchange } from './thread-events';
import { computeExchanges, isExcludedFromSections } from './thread-events';
import { getThreadEventsBump } from './threadActivity';
import { DEFAULT_CHAT_MODEL } from './models';
import { displaySection, EVENT_CHANNELS } from '../generated/thread-lifecycle';
import type { EventChannel, ArchiveState, DisplaySection } from '../generated/thread-lifecycle';
import { resetContentScroll } from '../hooks/useScrollMemory';
import type { Change, ChangelogRelease, CodingAgentModelValue, CodingAgentReasoningEffort, ReleaseNoticeView } from '../api/client';
import type { ReleaseCheck } from '../api/client/control';
import type { EnvironmentVariable, ModelInfo } from '../api/types';
import { markSwUpdateDismissed, markEngineVersionDismissed } from '../hooks/sw-update';

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
  | { type: 'plugin-install'; request: PluginInstallRequest; installed?: PluginInstallReceipt }
  | { type: 'plugin-uninstall'; request: PluginUninstallRequest; removed?: PluginUninstallReceipt };

/** The email confirmation panel, before or after the send. A present `sentAt`
 *  means the email went out and the panel is a read-only receipt. One field
 *  rather than a boolean plus a timestamp, so the two cannot desync. The
 *  marker lives on the form, which is persisted panel state, so a remount can
 *  never present the Send button for an already-sent email. */
export type EmailConfirmForm = Extract<InlineForm, { type: 'email-confirm' }>;

/** The plugin install panel, before or after the confirm. A present
 *  `installed` means the files landed and the panel is a read-only receipt.
 *  Same shape and the same reasons as `EmailConfirmForm`'s `sentAt`. A remount
 *  can never present an Install button for a staged `install_id` the engine
 *  has already popped. */
export type PluginInstallForm = Extract<InlineForm, { type: 'plugin-install' }>;

/** The plugin uninstall panel, before or after the confirm. `removed` present ⇒
 *  the files are gone and the panel is a read-only receipt. Mirrors
 *  `PluginInstallForm`. */
export type PluginUninstallForm = Extract<InlineForm, { type: 'plugin-uninstall' }>;

// --- Panel overlay (discriminated union replacing 6 independent signals) ---
export type PanelOverlay =
  | { type: 'form'; form: InlineForm }
  /** `fragment` is the app fragment (docs/glossary.md): the place inside the
   *  app a link named, delivered to the iframe as `location.hash`. It lives on
   *  the overlay so a target that outlives its app open is unrepresentable. */
  | { type: 'app-ui'; app: App; fragment?: string }
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
/** The app fragment of the open app, or null when the link named no target. */
export const currentAppFragment = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'app-ui' ? o.fragment ?? null : null;
});
export const previewFile = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'file-preview' ? o.path : null;
});

/** When true, file preview shows raw source instead of rendered output (for md, html, csv, svg). */
export const filePreviewSource = signal(localStorage.getItem('lucidos-file-preview-source') === 'true');

/** User override for the diff whole-file toggle. `null` means no explicit
 *  choice, so the effective view defaults by file status (see
 *  `diffWholeFileEffective`): an added file opens as the whole file,
 *  everything else on the unified hunks. Orthogonal to `filePreviewSource`,
 *  which still toggles source against rendered inside the whole-file view.
 *
 *  Transient. `store/effects.ts` resets it whenever the previewed file
 *  changes, so each new diff re-derives its default. Like
 *  `filePreviewEditing`, it is NOT persisted across diffs or reloads. */
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
 *  `DiffView` (see `fitsSideBySide`). Published here because the CONTENT
 *  pane's header needs the same answer: it offers the Side by side toggle only
 *  where the toggle would do something, and a present but inert control lies
 *  about what the surface can do.
 *
 *  Written from the measuring component rather than derived, because only the
 *  DOM knows. The content pane is resizable, so this is not a function of the
 *  viewport. Defaults to true so the first paint does not flash the unified
 *  view before the ResizeObserver has run. */
export const diffFitsSideBySide = signal(true);

/** When true, the data-file preview shows an editable textarea instead of the
 *  rendered or source view. `store/effects.ts` resets it whenever the previewed
 *  file changes, so a stale draft toggle never carries to a new file. Not
 *  persisted: editing is always an explicit, in-session action. */
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
 *  `viewNotification`'s miss path, where neither loaded list holds the row and
 *  a round-trip stands between the tap and the panel.
 *
 *  Deliberately NOT a `panelOverlay` variant. `panelOverlay` is the panel nav
 *  stack's unit of history, so a speculative write would leave a phantom entry
 *  that Back walks onto when the fetch fails. Keeping the pending state beside
 *  it means the overlay is only ever written with a real notification in
 *  hand. */
export const notificationDetailPending = signal<string | null>(null);

export function closeInlineForm(): void {
  // Trigger forms reset the list scroll on close (Save/Cancel/Escape) so the
  // user lands at the top instead of the row they just edited. Other form
  // types preserve their underlying view's scroll.
  const form = activeInlineForm.value;
  if (form?.type === 'trigger') resetContentScroll('triggers');
  panelOverlay.value = null;
}

/** Close `form`, but only while it is still the one on screen.
 *
 *  For any panel that resolves over an HTTP round trip: Escape dismisses during
 *  the request, so a bare `closeInlineForm()` afterwards would close whatever
 *  the user opened in the meantime. Identity comparison, not a type check, so a
 *  second staged request of the same kind is not mistaken for this one. */
export function closeInlineFormIfActive(form: InlineForm): void {
  if (activeInlineForm.value === form) closeInlineForm();
}

// --- Settings subview ---
export type SettingsSubview = 'main' | 'system' | 'models' | 'appearance' | 'memory' | 'devices' | 'accounts' | 'backup' | 'coding-agents' | 'locale' | 'marketplaces' | 'disk-usage' | 'permissions' | 'mcp' | 'keyboard-shortcuts' | 'access' | 'webhooks' | 'environment-variables' | 'thread-queue' | 'whats-new' | 'release-notices' | 'debugging' | 'communication-surfaces';
export type SettingsNavKey = Exclude<SettingsSubview, 'main'>;
export interface SettingsNavItem {
  key: SettingsNavKey;
  label: string;
  /** The header bar's form of `label`, when the full name does not fit there.
   *  The bar is the narrowest place a category name is shown: on a phone the
   *  title is one shrinkable member of a fixed-width cluster between two
   *  chevrons (`.header-title-cluster`, styles/header-mark.css). That leaves
   *  around a dozen characters, fewer at a raised UI scale.
   *
   *  Authored, never derived. A measured font-shrink is rejected, because a
   *  title that changes size from screen to screen makes the bar jitter as the
   *  user navigates. Both the iOS and Material top-bar conventions say the
   *  same thing: write a short title rather than scale a long one.
   *
   *  Only the bar reads it. The Settings home list, Search Everywhere, the
   *  history menu and the title's own tap-tooltip all keep the full `label`,
   *  so the shorthand never becomes the category's real name. */
  short?: string;
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
  // The subpanels lead with the two a user ARRIVES at rather than goes looking
  // for, and notices lead those: the badge and the notice modal both send the
  // reader here, and a notice is the only thing in Settings that asks for work.
  { key: 'release-notices', label: 'Release Notices', short: 'Notices' },
  // Next, because the Lucidos menu's version row opens it and the update notice
  // links to it. It also sits closest to what Overview already says, which
  // release is running and how to move it forward.
  { key: 'whats-new', label: "What's New" },
  { key: 'thread-queue', label: 'Thread Queue' },
  { key: 'backup', label: 'Backup' },
  { key: 'memory', label: 'Memory' },
  { key: 'disk-usage', label: 'Disk Usage' },
  { key: 'environment-variables', label: 'Environment Variables', short: 'Env Vars' },
  { key: 'debugging', label: 'Debugging' },
  { key: 'communication-surfaces', label: 'Communication Surfaces', short: 'Surfaces' },
];

// The top-level Settings categories, in home-list order. Two rules hold this
// list together (see
// docs/plans/2026-08-05-settings-information-architecture.md):
//
//  1. NO ENTRY IS PLATFORM-GATED. Every row here renders on every platform, so
//     the nav has one shape everywhere and "go to Settings → X" is true for
//     everyone. Platform gating belongs to a ROW or SECTION inside a category:
//     the iOS external-link target and the Tauri in-app browser toggle both
//     live inside Appearance & Behavior → Links that way. Gating a whole
//     CATEGORY hides a correctly-predicated row from the one platform it
//     applies to. Pinned by
//     components/settings/__tests__/settings-nav-structure.test.ts.
//  2. A ROW IS A CATEGORY, NOT A SETTING. A single control belongs in a
//     section of a bigger category, not as a peer of System's twelve.
//
// Groups are contiguous and rendered as headings. They add no tap depth.
export const SETTINGS_NAV_ITEMS: SettingsHomeNavItem[] = [
  { key: 'models', label: 'Models', group: 'Assistant' },
  { key: 'permissions', label: 'Permissions', group: 'Assistant' },
  // Beside Permissions, because the two answer the same question from
  // different ends: which tools the agent is offered, and which of those it
  // may call without asking. The page also owns the mcp-allowed-tools list,
  // which is meaningless without the server list next to it.
  { key: 'mcp', label: 'MCP Servers', group: 'Assistant' },
  // Binary paths plus registered repositories: everything a coding-agent
  // thread needs configured, in one place.
  { key: 'coding-agents', label: 'Coding Agents', short: 'Agents', group: 'Assistant' },
  { key: 'accounts', label: 'Accounts', group: 'Workspace' },
  // Language and timezone: workspace-wide user preferences, and among the most
  // looked-for settings there are.
  { key: 'locale', label: 'Locale', group: 'Workspace' },
  { key: 'marketplaces', label: 'Marketplaces', group: 'Workspace' },
  // Reaching this engine from elsewhere: the mobile-access guide plus the
  // engine's network bind.
  { key: 'access', label: 'Access', group: 'Workspace' },
  // Beside Access, because both answer "who reaches this machine from
  // outside". Access is the door a person comes through; this is the one a
  // third-party service posts to.
  { key: 'webhooks', label: 'Webhooks', group: 'Workspace' },
  { key: 'devices', label: 'Devices', group: 'Workspace' },
  { key: 'system', label: 'System', group: 'Workspace' },
  // Where a link opens is behaviour rather than display, so the label says
  // both. The KEY stays the head noun, matching the repo's own ampersand
  // precedent: "Chat & triggers" is anchored `models:chat`. Keeping
  // `appearance` also keeps every persisted search recent, the LLM's
  // `settings_view` value and the SDK type stable.
  { key: 'appearance', label: 'Appearance & Behavior', short: 'Appearance', group: 'This device' },
  { key: 'keyboard-shortcuts', label: 'Keyboard Shortcuts', short: 'Shortcuts', group: 'This device' },
];

function settingsNavItem(key: Exclude<SettingsSubview, 'main'>): SettingsNavItem | undefined {
  return [...SETTINGS_NAV_ITEMS, ...SETTINGS_SYSTEM_SUBPANEL_ITEMS].find(item => item.key === key);
}

/** The category's full name, for every surface with room for it. */
export function settingsSubviewLabel(key: Exclude<SettingsSubview, 'main'>): string | undefined {
  return settingsNavItem(key)?.label;
}

/** The category's name as the header bar shows it: the authored `short` when
 *  there is one, the full label otherwise. See `SettingsNavItem.short`. */
export function settingsSubviewShortLabel(key: Exclude<SettingsSubview, 'main'>): string | undefined {
  const item = settingsNavItem(key);
  return item && (item.short ?? item.label);
}

/** Where a retired subview key now lives. The content did not disappear, it
 *  moved, so each maps to the category that absorbed it rather than to `main`.
 *
 *  A `Map`, not an object literal, because the lookup key is UNTRUSTED: it
 *  comes from persisted JSON. An object literal inherits `Object.prototype`,
 *  so `obj['constructor']` returns a truthy function. The migration would hand
 *  that back as a subview, landing on the blank panel it prevents. */
const RETIRED_SETTINGS_SUBVIEWS = new Map<string, SettingsSubview>([
  // Both were one-control categories folded into Appearance & Behavior's Links
  // section. `appearance` itself is NOT retired: it kept its key.
  ['links', 'appearance'],
  ['experimental', 'appearance'],
  ['repositories', 'coding-agents'],
  ['mobile-access', 'access'],
  ['network-access', 'access'],
]);

/** Resolve a subview name that came from OUTSIDE this build into a renderable
 *  one: the persisted nav stack (`lucidos-nav-state`), or any other untrusted
 *  source.
 *
 *  The nav stack survives the upgrade that renames a subview, and
 *  `SettingsView.renderSubview` falls through to `null` for a key it no longer
 *  knows, so restoring the raw string lands the user on a BLANK Settings
 *  panel. A retired key maps to the category that absorbed it. Anything
 *  unrecognised falls back to the Settings home list. */
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
 *  collapsed onto `main`, since there is no user to tell and a blank panel is
 *  the alternative. A `NavigationRequested` from outside this build wants the
 *  alias applied but a genuinely unknown value REPORTED: the caller can be
 *  told, and a typo should not silently land on the Settings home. */
export function aliasRetiredSettingsSubview(raw: string): string {
  return RETIRED_SETTINGS_SUBVIEWS.get(raw) ?? raw;
}

// --- Active menu item ---
// Migrate older persisted state onto the top-level **Plugins** panel:
//  - the retired 'app-store' menu item;
//  - someone last viewing Apps → Store, the prior fold-in.
// The per-tab selection keys are retired with them. The panel now uses a
// single "Installed only" filter rather than Installed and Store tabs.
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
/** The engine's own name for this workspace: the last component of its
 *  directory, which for a gateway-provisioned workspace IS its *workspace
 *  address* (`docs/glossary.md`). This is the workspace's IDENTITY here: it is
 *  stable across renames, unique across the machine, and thread-ref links
 *  (`utils/threadRef.ts`) plus cross-workspace routing match on it. Do not swap
 *  it for the display label. */
export const workspaceName = signal<string>('');
/** The display label the workspace gateway's registry holds for this workspace,
 *  i.e. what the picker lists and what a rename edits. Empty until the control
 *  listing resolves, and stays empty with no gateway in front of us. Renaming is
 *  a registry write with no engine involvement, so the engine cannot report this
 *  and the app has to ask the gateway (see `actions/workspace-label.ts`). */
export const workspaceDisplayName = signal<string>('');
/** What every "which workspace is this" surface shows: the user's own label
 *  when we know it, else the engine's name. ONE derived value, so no screen
 *  can carry a different name from the switcher row beside it. The switcher
 *  reads the gateway listing, which is what a rename writes. */
export const visibleWorkspaceName = computed(
  () => workspaceDisplayName.value || workspaceName.value,
);
export const workspacePath = signal<string>('');
export const engineStartedAt = signal<string | null>(null);
export const lucidosRelease = signal<string | null>(null);
export const lucidosReleaseDirty = signal<boolean>(false);
/** Every published release's notes, for Settings > System > What's New. Loaded
 *  on the panel's mount rather than at startup: it is content the user asks
 *  for, and the whole changelog is a few hundred kilobytes.
 *
 *  The history you HAVE. A release the updater is OFFERING postdates the binary
 *  this came out of, so its notes arrive with the update check instead
 *  (`whatsNewOfferNotes`). */
export const changelogReleases = signal<Loadable<ChangelogRelease[]>>({ status: 'not-loaded' });
/** The release whose What's New this client has opened, from localStorage, or
 *  `null` when it never has. Drives the unread dot on the Lucidos menu's version
 *  row. Per CLIENT, not per account: "have I read this" is a fact about this
 *  browser, the same class as the diff-view and file-preview toggles above. */
export const whatsNewSeenRelease = signal<string | null>(
  localStorage.getItem('lucidos-whats-new-seen-release'),
);
/** The release the What's New panel was opened to READ, or `null` for an
 *  ordinary open. Written by `openWhatsNew` and consumed once by the panel, the
 *  same one-shot shape as {@link settingsScrollTarget}.
 *
 *  An update offer announces one specific release, and that is the one its
 *  What's new must open. Without this the panel expands the release you are
 *  RUNNING, which is the older one and the opposite of what an offer is
 *  about. */
export const whatsNewTargetRelease = signal<string | null>(null);
/** Every *release notice* this release has reached, with the id of the one the
 *  workspace still owes an answer to.
 *
 *  Loaded at startup rather than on a panel open, unlike the changelog above:
 *  the modal is the point, and it has to be able to raise itself. Small by
 *  construction, since most releases carry no notice at all.
 *
 *  Per WORKSPACE, not per client. Answering on the laptop settles it on the
 *  phone too, which is the opposite of {@link whatsNewSeenRelease} and is
 *  deliberate: a notice asks for work on the workspace, done once. */
export const releaseNoticeView = signal<Loadable<ReleaseNoticeView>>({ status: 'not-loaded' });
export const engineVersion = signal<string | null>(null);
export const latestEngineVersion = signal<string | null>(null);
export const latestTauriAppVersion = signal<string | null>(null);
/** What is in the packaged update being offered, as raw markdown, or `null` when
 *  none is offered or its manifest carries no notes.
 *
 *  The only way this client can say what a PENDING update contains. The
 *  offered version postdates the engine binary running here, so the baked
 *  changelog `changelogReleases` holds does not carry it. Falling back to that
 *  would show the notes for the version already installed. Written only by the
 *  update check, beside `latestTauriAppVersion`, so the two cannot describe
 *  different releases. */
export const latestTauriAppNotes = signal<string | null>(null);
/** The gateway's release check (ADR 0108), or `null` when it is unknown.
 *
 *  Null covers three cases the UI treats alike: the status poll has not landed,
 *  the gateway is older and omits the field, or there is no gateway at all (a
 *  direct engine port). In every one of them there is no offer, which is the
 *  ADR 0105 degradation.
 *
 *  The check itself is machine-global and lives in the gateway, so this is one
 *  answer shared by every open window rather than a per-window poll. */
export const releaseCheck = signal<ReleaseCheck | null>(null);
/** Why the last packaged app-update check failed, or `null` when it succeeded
 *  or has not run. Rendered in Settings → System so a failing check is
 *  DIAGNOSABLE instead of silent. Swallowed, it makes a stranded install
 *  indistinguishable from an up-to-date one. */
export const appUpdateCheckError = signal<string | null>(null);
/** True while a USER-INITIATED update check is in flight.
 *
 *  Only the click sets it, never the background poll: the poll runs on mount
 *  and on resume, and reporting it would make the button flicker on its own.
 *
 *  The control the user pressed reads it for its label and its `disabled`. A
 *  check costing a network round-trip cannot then look like a dead button. See
 *  `checkForUpdatesNow` in store/actions/app-update.ts. */
export const appUpdateCheckInFlight = signal(false);
/** Live phase of a packaged app-update run, or `null` when none is in flight.
 *  Fed by the `app-update-progress` Tauri event (store/actions/app-update.ts).
 *  The engine has no part in it, so this stays null in a browser, PWA or dev
 *  client. Read by BOTH the progress dialog and Settings → System, so the two
 *  cannot disagree about what the update is doing.
 *
 *  Typed to the IN-FLIGHT frames only. `cancelled` and `failed` end a run, and
 *  the handler clears this signal rather than storing them. A terminal frame
 *  parked as live state is therefore not representable. */
export const appUpdateProgress = signal<AppUpdateRunning | null>(null);
/** True when the packaged update has passed the point of no return: the bundle
 *  is being swapped or the stack restarted, which kills the gateway under the
 *  page. The resulting connection and SSE failures would bury the narration,
 *  so they are suppressed exactly as `engineRestarting` suppresses them for a
 *  restart. Only the update's own toast (`showWhileUnavailable`) stays up. */
export const appUpdateCommitted = computed(() => {
  const phase = appUpdateProgress.value?.phase;
  return phase === 'installing' || phase === 'restarting-services' || phase === 'relaunching';
});
/** Can the engine reach its own database? Mirrored from `/health`'s
 *  `database_reachable` by the connection poll; an older engine omits the field,
 *  which reads as `true`, so nothing changes for one.
 *
 *  An engine outlives its database, most commonly when a dev quits Docker
 *  Desktop. It keeps answering `/health` and streaming SSE while every query
 *  behind it fails. Without this signal each of the ~20 startup loads reports
 *  that separately. The boot splash then waits out its safety cap on a thread
 *  list that can never arrive. See ADR 0037 and `engine::db_health`. */
export const databaseReachable = signal(true);
/** True when the connected engine is a packaged desktop build. Routes the
 *  "Restart" control (LaunchAgent kickstart vs. dev rebuild script) and gates
 *  the Tauri-only half of the Settings Access page. Set from /health. */
export const enginePackaged = signal<boolean>(false);

/** False when the connected engine booted with no LLM provider configured, the
 *  UnconfiguredProvider sentinel a packaged build's first run leaves. Drives
 *  the first-run provider onboarding in the welcome surface. Set from /health,
 *  and defaults to `true` so onboarding never flashes before the first
 *  probe. */
export const llmConfigured = signal<boolean>(true);

/** Provider backends the connected engine actually has configured, from
 *  /health. Filters the chat model picker to providers the user has set up.
 *  `null` means do not filter, which is the safe default so the picker is
 *  never empty before the first probe or under mock. */
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

// --- Toast placement (temporary: a shape comparison, see docs/temporary-measures.md) ---

/** Where the toast stack sits and what a toast is shaped like.
 *
 *   - `bottom-right`: one stack in the window's bottom-right corner, the spot
 *     VS Code and IntelliJ both default to. Anchored to the WINDOW rather than
 *     a pane, so it holds still through a divider drag and never meets the seam.
 *   - `top-bleed` / `bottom-bleed`: ONE stack spanning both panes, edge to edge.
 *     A full-bleed bar COVERS the pane divider for its whole width, where a
 *     narrower card straddles it with the seam showing above and below.
 *   - `card`: one stack, the toast still a 30rem card, centred on the viewport.
 *     This is the shape the per-pane columns replaced.
 *   - `pane`: today's behaviour, one column per visible pane.
 *
 *  The shapes exist only so they can be compared in the running app. Exactly
 *  one survives, and the picker goes with the losers. */
export type ToastPlacement = 'bottom-right' | 'top-bleed' | 'bottom-bleed' | 'card' | 'pane';

const TOAST_PLACEMENTS: readonly ToastPlacement[] = [
  'bottom-right', 'top-bleed', 'bottom-bleed', 'card', 'pane',
];

/** The one gate on the value, shared by the stored-value read below and the
 *  picker. Both need it, and a second hand-written list of the same strings
 *  would let a value be storable but unpickable, or the reverse. */
export function isToastPlacement(value: string | null): value is ToastPlacement {
  return value !== null && TOAST_PLACEMENTS.includes(value as ToastPlacement);
}

/** Device-local, like the animation-speed slider beside it: this is a diagnostic
 *  for judging a shape, not a preference anyone should have to keep in sync
 *  across devices. An unrecognised stored value falls back to the default rather
 *  than reaching the CSS as an attribute matching no rule. */
function storedToastPlacement(): ToastPlacement {
  const raw = localStorage.getItem('lucidos-toast-placement');
  return isToastPlacement(raw) ? raw : 'bottom-right';
}

export const toastPlacement = signal<ToastPlacement>(storedToastPlacement());

/** What every animated duration is MULTIPLIED by: the reciprocal of the speed,
 *  so 10x speed is a 0.1 scale. It exists beside the multiplier, rather than
 *  each caller writing `1 / speed`, so one name root crosses both layers:
 *
 *    - CSS reads it as `var(--duration-scale)`, published onto :root by
 *      store/effects.ts and folded into every `--duration-*` token in
 *      styles/global/base.css. That is what lets the slider reach a plain CSS
 *      transition at all.
 *    - TS reads it through `scaledDurationMs` for a timer that must outlive
 *      one of those transitions, and directly for a Web Animations duration
 *      (useFlipAnimation).
 *
 *  1 at the slider's centre, so a user who never touches it sees today's
 *  timings exactly. */
export const durationScale = computed(() => 1 / speedMultiplier.value);

/** A base duration in ms, scaled to the current animation speed.
 *
 *  For a TS timer that MIRRORS a CSS duration, such as keeping an element
 *  mounted through its own fade. Pass the 1x duration of the CSS it mirrors.
 *  Add any safety slack OUTSIDE the call, since slack is a fixed margin rather
 *  than animation. So `scaledDurationMs(PANE_TRANSITION_MS) + 100` is the
 *  shape, never `scaledDurationMs(PANE_TRANSITION_MS + 100)`.
 *
 *  Scaling the CSS without scaling these desyncs the pair. At 0.1x the
 *  drawer's width transition runs 3s while an unscaled 350ms timer unmounts
 *  its list, so the drawer blanks and then slides shut empty. */
export function scaledDurationMs(baseMs: number): number {
  return baseMs * durationScale.value;
}

// --- Threads ---
export const threadDrawerOpen = signal(
  localStorage.getItem('lucidos-thread-drawer-open') === 'true'
);
/** The active drawer view, picked from the drawer view selector and persisted
 *  across reloads. Five mutually-exclusive views:
 *    - `all`: the default sectioned list (Current/Saved/Archive).
 *    - `attention`: the agent is stuck and the user must act, awaiting an
 *      answer or permission, or holding a failed turn (`threadNeedsAttention`).
 *    - `review`: carrying a change ready to apply (`threadInReview`).
 *    - `running`: actively working on a response (`threadIsRunning`).
 *    - `drafts`: threads with an unsent draft (`draftThreads`).
 *
 *  The badge counts are recomputed from the rehydrated `threadMap`, so only
 *  the active SELECTION is persisted. One key stores the choice and encodes
 *  the one-active invariant. Any unknown value falls back to `all`. */
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
export const THREAD_DRAWER_WIDTH_KEY = 'lucidos-thread-drawer-width';

// Clamp at load: a width persisted under a smaller root font size would
// otherwise render an overflowing header until the next drag corrects it. The
// floor itself lives in store/paneMinimums.ts, alongside the two split-pane
// floors it has to be weighed against.
export const threadDrawerWidth = signal(
  Math.max(
    minDrawerWidth(),
    Number(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)) || DEFAULT_DRAWER_WIDTH,
  )
);

/** Re-apply the floor after something moved it. The floor derives from the root
 *  font size, so a UI-scale change can leave a settled drawer below it.
 *  `applyUiScale` calls this for the same reason it re-measures the scrollbar
 *  gutter. Widening persists, so the next reload starts from the corrected
 *  width rather than re-correcting every boot.
 *
 *  Stays HERE rather than moving to paneMinimums.ts with the floor it reads.
 *  It mutates `threadDrawerWidth`, and this module's init imports that one, so
 *  the import back would be a boot-time cycle. */
export function clampThreadDrawerWidth(): void {
  const min = minDrawerWidth();
  if (threadDrawerWidth.value >= min) return;
  threadDrawerWidth.value = min;
  localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(min));
}
export const focusedThreadId = signal<string | null>(
  localStorage.getItem(FOCUSED_THREAD_KEY)
);

/** Single setter that keeps focusedThreadId and FOCUSED_THREAD_KEY in lockstep.
 *  Every production-code mutation of focusedThreadId goes through here, so the
 *  next reload resumes the same thread. That matters most for a compose draft,
 *  whose id is allocated client-side and never reaches the server until a
 *  Send. Idempotent, because hot-path callers fire with the same id
 *  repeatedly and the storage write is synchronous. */
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

// One-shot ticket, set on a compose-to-active send. It tells the ThreadPane
// FLIP to animate the textarea height collapse together with the position
// slide, rather than letting the textarea snap short first. The FLIP consumes
// it and owns the height reset in every exit path, so it cannot stick tall.
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

/** The persisting writer for the channel selection, matching
 *  `setSelectedTriggerIds` / `setSelectedRepoIds` / `setSelectedAppIds` below.
 *  Every write goes through here so no caller can set the signal and forget the
 *  localStorage half, which is how a filter survives a reload. */
export function setThreadChannelFilter(next: Set<ThreadChannel>): void {
  threadChannelFilter.value = next;
  localStorage.setItem(THREAD_CHANNEL_FILTER_KEY, JSON.stringify([...next]));
}

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
// cc_repo_ids only. Mirrors `selectedTriggerIds`: the dropdown turns the
// Coding Agent parent indeterminate when this set is non-empty.
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

// Mirrors selectedRepoIds for app coding-agent threads. Apps sit beside repos
// under the Coding Agent parent in the filter dropdown, and their selection
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

// Whether the filter dropdown lists deleted trigger, repo and app options:
// those whose entity is gone but which threads still reference. Default OFF,
// so a deleted entry is excluded unless the user opts in. A SELECTED deleted
// option stays visible regardless, so a restored filter is clearable.
const INCLUDE_DELETED_FILTERS_KEY = 'lucidos-filter-include-deleted';

export const includeDeletedFilterOptions = signal<boolean>(
  localStorage.getItem(INCLUDE_DELETED_FILTERS_KEY) === 'true'
);

export function setIncludeDeletedFilterOptions(next: boolean): void {
  includeDeletedFilterOptions.value = next;
  localStorage.setItem(INCLUDE_DELETED_FILTERS_KEY, String(next));
}

// Every trigger, repo and app that has a thread, from
// /api/v1/threads/filter-facets. Seeds the drawer "Show" dropdown so it lists
// all of them rather than only those in the loaded window. Refreshed on
// startup and after each full thread reload.
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
 *   - The bootstrap focuses the thread OPTIMISTICALLY, so the pane moves on
 *     the tap and `ThreadView` renders its delay-gated skeleton rather than a
 *     dead interval.
 *   - `ThreadView` clears a `focusedThreadId` whose thread is not in the map
 *     once `threadsLoaded` is true. It must NOT do that to a thread whose
 *     metadata is still in flight, or the optimistic focus would be undone on
 *     the next render. This signal is the exemption. */
export const bootstrappingThreadId = signal<string | null>(null);
/** Thread IDs whose title was set by a ThreadTitleGenerated event (authoritative). */
export const generatedTitleIds = new Set<string>();
/** Whether the server has more older threads to load (infinite scroll). */
export const threadHasMore = signal(true);
/** Whether a load-more request is currently in flight. */
export const threadLoadingMore = signal(false);
/** Total size of the archived pile from the backend, refreshed by every
 *  `loadAllThreads`. Drives the collapsed Archive section's count badge, so it
 *  shows the true total rather than the loaded window. A plain signal rather
 *  than a `Loadable`, matching the `threadMap` and `threadsLoaded` the same
 *  fetch populates. The badge falls back to the loaded count until it
 *  lands. */
export const archiveThreadCount = signal(0);
/** Derive effective thread status, accounting for in-progress apply operations and pending messages. */
export function effectiveThreadStatus(thread: ThreadState): ThreadStatus {
  // Archive = user acknowledged any prior failed/aborted state. Drop the red
  // status dot immediately, before the ThreadArchived SSE round-trip lands.
  if (archivingThreadIds.value.has(thread.meta.id)) return 'idle';
  // Apply is deliberately NOT an optimistic running flip. The thread stays in
  // Review while the backend finishes a clean fast-path apply, or wakes the
  // agent for harden or merge-conflict resolution. The agent's own activity
  // events then transition the status. WaitingBanner's disabled "Apply..."
  // button is the visual feedback.
  //
  // A pending user message means the request was sent and the thread is
  // running ahead of the SSE event. An `unconfirmed` row is excluded: the
  // safety refetch gave up on it, so it is kept only to keep the text visible
  // and no turn is in flight behind it. Counting it would pin the thread on
  // 'running' for the life of the page.
  if (thread.pendingUserMessages.some(p => !p.unconfirmed)) return 'running';
  return thread.meta.status;
}

/** True for the cancellable statuses: a turn is in flight, or paused on a user
 *  question. The two transition through each other via `UserQuestionAnswered`,
 *  so almost every mid-turn check needs both. */
export function isMidTurn(status: ThreadStatus): boolean {
  return status === 'running' || status === 'waiting_for_user_answer';
}

/** True when the agent is not producing output. Future events will not resolve
 *  trailing Thinking spinners in non-current exchanges, so the renderer must
 *  clean them up itself.
 *
 *  NOT the inverse of `isMidTurn`. `waiting_for_user_answer` belongs to BOTH
 *  sets: mid-turn cancellable, but quiescent for output. That overlap is the
 *  bug surface this addresses. A `CodingAgentPromptSent` for a follow-up the
 *  agent paused before consuming leaves a Thinking step nothing can resolve.
 *  The resume events attach to the new `UserQuestionAsked` exchange by
 *  current-pointer routing, not to the stranded one. */
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
 *  Without this carve-out the answer-to-resume gap mislabels the answered turn
 *  as aborted. The backend flips the projection to `running` on
 *  `UserQuestionAnswered`, but the client's `meta.status` advances only when a
 *  per-event aggregate carrying `running` arrives. The resume's first events
 *  can land while the snapshot still reads `waiting_for_user_answer`. The
 *  answered divider then has steps, no terminal and `threadIdle`, and the
 *  stale detector in `exchange-render.ts` flashes "Aborted" until the
 *  aggregate lands. An unknown thread falls back to treat-as-active. */
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

/** Threads in the Current drawer section, ignoring the active filter.
 *  `displaySection` ignores `meta.state`, so the drawer-hidden carve-out is
 *  applied here too. This is the UNFILTERED section membership. The
 *  archive-next-focus picker walks the drawer's filter-aware render order
 *  (`orderedCurrentForReview`), so it lands only on a visible thread. */
export function getCurrentThreads(): ThreadState[] {
  const result: ThreadState[] = [];
  for (const thread of threadMap.value.values()) {
    if (isExcludedFromSections(thread)) continue;
    if (getThreadDisplaySection(thread) === 'current') result.push(thread);
  }
  return result;
}

/** Whether a thread needs the user's attention. It sits in the Current or
 *  Saved section AND the agent is stuck waiting on the user: a question or a
 *  permission request, both of which surface as `waiting_for_user_answer`, or
 *  a failed turn. This is the "nothing progresses until you act" subset, where
 *  a change merely READY to apply is `threadInReview`, a separate view.
 *
 *  Both statuses are mutually exclusive with `running`, so no running guard is
 *  needed. A composing or discarded thread never qualifies. Shared by
 *  `attentionThreadCount` and `attentionThreads`, so the badge and the
 *  filtered list cannot disagree. */
export function threadNeedsAttention(thread: ThreadState): boolean {
  if (isExcludedFromSections(thread)) return false;
  const section = getThreadDisplaySection(thread);
  if (section !== 'current' && section !== 'saved') return false;
  const status = effectiveThreadStatus(thread);
  // `paused` is deliberately absent, and precisely so: the backend writes it
  // for exactly one shape, the user's own Switch to new version, which the
  // engine resumes by itself within seconds. Nothing progresses BECAUSE of the
  // user there, so counting it would flash the badge on every switch.
  //
  // Every OTHER interruption is written `failed`, including a boot that could
  // not keep that resume promise. Those land in the arm below with a Continue
  // button. A paused thread still floats to the top of Current with its own
  // dot via `reviewTier`.
  //
  // Both verdicts are now written whether or not a change is pending. So a
  // coding-agent turn that failed with one counts here, instead of hiding
  // behind the change. It is in the Review view too, since `threadInReview`
  // only excludes a RUNNING thread.
  return status === 'waiting_for_user_answer' || status === 'failed';
}

/** Whether a thread is ready for review. It sits in the Current or Saved
 *  section AND carries a coding-agent change ready to apply
 *  (`codingAgentProposed`). A `running` thread is excluded, because a proposed
 *  change whose follow-up turn is in flight is not yet READY: its
 *  WaitingBanner shows Cancel rather than Apply. That mirrors
 *  `getCodingAgentWaitingInfo`'s running guard, so the badge cannot claim a
 *  thread with no Apply button showing.
 *
 *  Independent of `threadNeedsAttention`. A thread both awaiting an answer and
 *  carrying a proposed change legitimately surfaces in both views. */
export function threadInReview(thread: ThreadState): boolean {
  if (isExcludedFromSections(thread)) return false;
  const section = getThreadDisplaySection(thread);
  if (section !== 'current' && section !== 'saved') return false;
  if (effectiveThreadStatus(thread) === 'running') return false;
  return thread.meta.codingAgentProposed;
}

/** Count of threads where the agent is stuck waiting on the user (see
 *  `threadNeedsAttention`), across the Current and Saved sections. Drives the
 *  selector's needs-attention badge. */
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

/** Whether a thread is actively working. It sits in the Current or Saved
 *  section AND its effective status is `running`, the state the status dot
 *  labels "Running".
 *
 *  A `running` thread always routes to Current or Saved (`displaySection`), so
 *  the section gate never drops one. It only keeps the composing and discarded
 *  carve-out in lockstep with the sibling predicates. Independent of
 *  `threadNeedsAttention` and `threadInReview`, which both exclude `running`,
 *  so the three views never claim the same thread. */
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
  // Subscribe to ONLY this thread's events bump, so another thread's
  // streaming does not fan out here. `threadMap.peek()` then reads the map
  // without a wide subscription. The bump fires on every event arrival for
  // this thread, including the SSE skeleton-create path, so the computed
  // catches a freshly-inserted thread as soon as its first event lands.
  getThreadEventsBump(id);
  const thread = threadMap.peek().get(id);
  if (!thread) return [];
  return computeExchanges(thread);
});

export const activeStreamingBuffer = computed(() => {
  const id = focusedThreadId.value;
  if (!id) return '';
  // Per-thread bump subscription, as in `activeExchanges` above.
  // `streamingBuffer` mutates per token, which is what bumpThreadEvents fires
  // on, so the live token stream lands here.
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

// True when the prompt is in the centered "compose" layout: either the blank
// view, with no focused thread and no exchanges, or a focused composing draft.
// One source of truth for ThreadPane's FLIP and PromptInput's height
// animation, so they agree by construction. The FLIP fires only when this
// CHANGES; the height animation runs only while it STAYS true.
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
// Persisted in localStorage so the last-viewed pane survives the PWA being
// killed. Reopening lands on the pane the user left rather than a forced reset
// to 'thread'. Safe because the content pane's own content is independently
// restored from the localStorage nav stack (navigation.ts `restoreState`), so
// a restored 'content' pane is never blank.
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
 *  persist effect lives in `effects.ts`. Send and discard deliberately do NOT
 *  reset it: the choice sticks, so the next fresh compose keeps the mode the
 *  user picked. */
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
    // Startup probe the user did not initiate, and no toast or Loadable
    // surface exists at module-load time. Self-recovery: the next toggle click
    // fires the persist effect in effects.ts and overwrites the bad payload.
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
// Where a coding-agent thread should run: the coding half of the compose
// destination (see store/composeDestination.ts). The destination picker writes
// here via `applyDestination`. chat.ts resolves it to the engine's `folder`
// request field, and compose.ts binds the resolved values onto the promoted
// thread's `meta`.
export type Scope =
  | { kind: 'lucidos' }
  | { kind: 'external'; repoId: string }
  | { kind: 'app'; appId: string };

const SCOPE_STORAGE_KEY = 'lucidos-coding-agent-last-scope';
// Legacy key names. Read once, migrated to SCOPE_STORAGE_KEY, then deleted, so
// a long-lived PWA keeps the user's compose destination.
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
    // Corrupt value: fall through to migration or default.
  }
  return null;
}

function restoreScope(): Scope {
  const current = parseStoredScope(localStorage.getItem(SCOPE_STORAGE_KEY));
  if (current) return current;

  // One-time migration from the pre-rename key: read it, rewrite under the new
  // key, and delete the old one, so the next reload reads the new shape.
  if (localStorage.getItem(LEGACY_SCOPE_STORAGE_KEY) !== null) {
    const renamed = parseStoredScope(localStorage.getItem(LEGACY_SCOPE_STORAGE_KEY));
    localStorage.removeItem(LEGACY_SCOPE_STORAGE_KEY);
    if (renamed) {
      localStorage.setItem(SCOPE_STORAGE_KEY, JSON.stringify(renamed));
      return renamed;
    }
  }

  // Older one-time migration from a bare repo string, where '' meant Lucidos
  // and any other value was an external repo UUID.
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

/** The account DEFAULT coding agent: the SEED for a fresh compose's backend
 *  chip, from the `coding_agent_default` preference.
 *
 *  A per-draft pick lives in `composeSelections` and does NOT write back here,
 *  so changing the chip on one draft never changes another or this default.
 *  `resolveCodingAgent` falls back here for an override-less draft.
 *  `sendCompose` binds the result onto the thread's meta at promotion. See ADR
 *  0006 for the workspace-scoped default. */
export const selectedCodingAgent = signal<import('../api/types').CodingAgent>('claude-code');

/** Translate a Scope into the engine's `folder` request field.
 *  Lucidos gives the empty string, which the engine defaults to Lucidos.
 *  External gives the repo UUID, which `resolve_folder_input` looks up.
 *  App gives a workspace-relative path, which `classify_resolved_folder`
 *  matches to the app branch. */
export function scopeToFolder(scope: Scope): string {
  switch (scope.kind) {
    case 'lucidos': return '';
    case 'external': return scope.repoId;
    case 'app': return `data/apps/${scope.appId}`;
  }
}

/** Read the repo UUID out of a Scope when one applies, for a path that has to
 *  surface the bound repo. App and Lucidos return undefined, and the caller
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
 *  selection alive, yanking a user who had scrolled away. It would also fight
 *  a shift-click that extends a selection upward. Mirrors
 *  `pluginScrollTarget`. */
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
 *  These arrive from outside the app, so anything that is not a positive whole
 *  number is rejected rather than trusted. A fractional or negative line would
 *  index a row that does not exist, and a non-number would render as `NaN` in
 *  the highlight comparison. An inverted range is swapped rather than dropped,
 *  since the author's intent is unambiguous.
 *
 *  Whether the range fits INSIDE the file is deliberately NOT checked. The
 *  line count is unknown until the content loads, and a range past the end
 *  highlights nothing while the file still opens. */
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
 *  A union rather than one shape with two optional fields, because the two
 *  modes need DIFFERENT qualifiers. A `file` names a git revision, a `diff`
 *  names the Change whose hunks to show, and neither is meaningful in the
 *  other's mode. */
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
 *  `diff#<changeId>`) rather than added as a fourth colon-separated field. It
 *  therefore survives nav history persistence AND stays unambiguous: a git ref
 *  cannot contain a colon (`git check-ref-format`) and a path can, so the
 *  "everything after the third colon is the path" rule still holds. Without
 *  the embedded changeId, reloading on a diff view spins forever, because
 *  repoDiff is runtime-only state.
 *
 *  Takes the parsed shape so this is the exact inverse of `parseRepoPath`:
 *  `encodeRepoPath(parseRepoPath(s)) === s` for every locator `s` that
 *  parses. */
export function encodeRepoPath(locator: RepoLocator): string {
  const qualifier = locator.mode === 'file' ? locator.ref : locator.changeId;
  const modeSeg = qualifier ? `${locator.mode}#${qualifier}` : locator.mode;
  return `repo:${locator.repoId}:${modeSeg}:${locator.path}`;
}

/** Decode a repo file path from the panel overlay, or null when it is not one.
 *
 *  Four forms, all of them live:
 *
 *    repo:<repoId>:file:<path>              the clone's current HEAD
 *    repo:<repoId>:file#<ref>:<path>        that branch, tag or sha
 *    repo:<repoId>:diff#<changeId>:<path>   a Change's diff
 *    repo:<repoId>:diff:<path>              legacy, changeId-less, still parses
 *
 *  Every segment must be non-empty. This is the single predicate deciding "is
 *  this a repo path" for `normalizeDataPath`, `ContentPane`'s routing and
 *  `openEncodedRepoFilePreview`, and `file_path` reaches it from OUTSIDE the
 *  app. A structurally incomplete encoding like `repo::file:x` would otherwise
 *  parse into an empty repoId, path or ref. That opens a preview which can
 *  only 404, instead of falling back to the data-path preview.
 *
 *  The qualifier is sliced at the FIRST `#`, so a ref that itself contains one
 *  survives intact (`#` is legal in a ref name, unlike `:`). That also keeps a
 *  GitHub-style `#L510` line suffix working, since `parseRepoFileHref` strips
 *  it before the locator reaches here. */
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

/** Effective whole-file view state for the current diff preview, resolving the
 *  `diffWholeFile` override against a per-file default. An ADDED file defaults
 *  to the whole-file view, since its diff is all additions and unified hunks
 *  would just prefix every line with `+`. A modified or deleted file defaults
 *  to the hunks. An explicit header toggle writes a boolean and wins until the
 *  previewed file changes.
 *
 *  Derived from file status rather than stamped at open time, so it stays
 *  correct after a reload, where `repoDiff` re-populates asynchronously under
 *  a nav-restored overlay. */
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
/** Whether an "Apply All" batch is running. Drives the busy state of the bulk
 *  Apply All and Discard All buttons. Set optimistically on the click, and by
 *  the ApplyAllBatchStarted SSE event so a batch started on another device
 *  disables the buttons here too. Cleared by ApplyAllBatchCompleted.
 *
 *  The batch applies the first change synchronously and drives the rest in the
 *  background, including a multi-minute wait while it hardens an unhardened
 *  member. Without this the button reads as dead the whole time. */
export const applyAllInProgress = signal(false);
/** Thread IDs where archive is in progress (prevents duplicate API calls). */
export const archivingThreadIds = signal<Set<string>>(new Set());
/** Thread IDs whose change discard is in progress: hides Apply, shows "Discard...". */
export const discardingCCThreadIds = signal<Set<string>>(new Set());
/** Thread IDs where Cancel was clicked while an exchange is active. Disables
 *  the Cancel button (shows "Cancel...") and drives the spinner status label.
 *  Cleared when the thread leaves active status (via PromptInput effect). */
export const cancelingThreadIds = signal<Set<string>>(new Set());
/** Queued chat messages being removed optimistically, keyed by thread + event id. */
export const removingQueuedMessageIds = signal<Set<string>>(new Set());
export const queuedMessageRemovalKey = (threadId: string, messageId: string): string => `${threadId}:${messageId}`;
/** Thread IDs whose pending question was just answered, where the agent's
 *  resume has not yet moved the client's `meta.status` off
 *  `waiting_for_user_answer`. `isRenderedThreadIdle` reads it to suppress the
 *  "Aborted" flash in that gap. Set in the `answerThreadQuestion` action, and
 *  cleared once the real status moves or the answer fails. */
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
 *  resolved `Change` only. A caller that has to tell loading from failed reads
 *  `lazyChanges.value.get(id)` directly. */
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
/** Every change id currently being applied: the single source of truth,
 *  combining `applyingChangeIds` and `applyingNowThreadIds`. */
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
/** Thread IDs that own a pending change currently being applied: the reverse of
 *  `busyChangeIds`, mapping change-level apply tracking back onto its
 *  originating thread. That lets the focused thread's WaitingBanner show
 *  "Apply..." when its change is applied from the Changes panel, mirroring the
 *  in-thread Apply Now path, which is thread-side to begin with. */
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
 *  commit subjects that were merged. Listed grouped in the restart confirm
 *  dialog, which is what the user answers before the restart begins. */
export interface RestartGroup {
  threadId: string;
  threadTitle: string;
  commits: string[];
}

/** Applied changes requiring engine restart, grouped by originating thread. */
export const restartGroups = signal<RestartGroup[]>([]);
/** The two transcript-wide *turn controls*, both ON unless the reader turned
 *  them off: a turn shows its full response and its step log by default.
 *
 *  Storage holds only the DEVIATION, which is what makes that a real default
 *  rather than one only a fresh browser profile ever sees. The superseded keys
 *  were written on every load, clicked or not, so they record the old default
 *  and nobody's intent. There is nothing in them to migrate, so the seeds read
 *  `-v2` keys instead.
 *
 *  `persistTurnControl` is the other half, and it keeps the rename from being
 *  needed again: an ON control stores nothing at all, so a stored value always
 *  means the reader turned something off. */
export const STEPS_EXPANDED_KEY = 'lucidos-steps-expanded-v2';
export const DETAILS_EXPANDED_KEY = 'lucidos-details-expanded-v2';

/** Absent means ON, and so does any value `persistTurnControl` never wrote:
 *  `'false'` is the only thing that hides anything, so a corrupted key shows
 *  more rather than less. */
export function seedTurnControl(stored: string | null): boolean {
  return stored !== 'false';
}

/** The write half of `seedTurnControl`: an OFF control is recorded, an ON one
 *  clears the key. Called from an effect that runs on load as well as on a
 *  click, which is exactly why the ON branch must not write. */
export function persistTurnControl(key: string, on: boolean): void {
  if (on) localStorage.removeItem(key);
  else localStorage.setItem(key, 'false');
}

export const stepsExpanded = signal(seedTurnControl(localStorage.getItem(STEPS_EXPANDED_KEY)));
export const detailsExpanded = signal(seedTurnControl(localStorage.getItem(DETAILS_EXPANDED_KEY)));
// Clear the superseded pair rather than leaving a `false` under a name so
// close to the live one. Nothing reads it, and the next person looking at this
// app's storage would read it as the state of a control that is on.
//
// A one-shot purge, so it is removable: docs/temporary-measures.md
// § "Superseded turn-control localStorage keys cleared at load" holds the
// condition, and says the `-v2` names and the deviation-only write stay.
localStorage.removeItem('lucidos-steps-expanded');
localStorage.removeItem('lucidos-details-expanded');

/** Signal + toggle + expand triple, backed by a localStorage-persisted Set of
 *  "threadId:userSeq" keys.
 *
 *  `expand` is the one-way half, and it is one-way on purpose: a fold is an
 *  explicit act, so something else may lift it but nothing may impose it. Its
 *  callers are the transcript-wide reveals in the response header, which draw
 *  nothing on a folded turn. Turning one ON lifts the fold on the turn it was
 *  clicked from. Turning one off never re-folds anything. */
function createCollapsedStore(storageKey: string) {
  const sig = signal<Set<string>>(loadStringSet(storageKey));
  function toggle(threadId: string, userSeq: number): void {
    const key = `${threadId}:${userSeq}`;
    const next = new Set(sig.value);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    sig.value = next;
  }
  function expand(threadId: string, userSeq: number): void {
    const key = `${threadId}:${userSeq}`;
    if (!sig.value.has(key)) return;
    const next = new Set(sig.value);
    next.delete(key);
    sig.value = next;
  }
  return [sig, toggle, expand] as const;
}

function loadStringSet(storageKey: string): Set<string> {
  try {
    const saved = localStorage.getItem(storageKey);
    return saved ? new Set(JSON.parse(saved) as string[]) : new Set();
  } catch {
    return new Set();
  }
}

export const [collapsedExchanges, toggleExchangeCollapsed, expandExchange] =
  createCollapsedStore('lucidos-collapsed-exchanges');
export const [collapsedInitiators, toggleInitiatorCollapsed] =
  createCollapsedStore('lucidos-collapsed-initiators');

// --- Artifacts ---
export const artifacts = signal<Loadable<string[]>>({ status: 'not-loaded' });
/** Cache-buster for the open file preview, and the data-relative path it
 *  belongs to. The preview appends it to its URL. That URL is the `src` of the
 *  `<video>` / `<audio>` / `<img>` / iframe it renders, so a bump reloads
 *  whatever is playing.
 *
 *  The path is what makes that safe. A bare counter here was bumped by every
 *  `loadArtifacts()`, so a write to ANY file under `data/` restarted a video
 *  mid-playback. Only `invalidateFilePreview` writes this, and only for the
 *  file on screen.
 *
 *  Kept per path rather than reset on each write. A stamp falling back to 0
 *  when a different file changed would itself be a URL change. */
export const filePreviewRevision = signal<{ path: string; rev: number } | null>(null);
/** The cache-buster owed to whatever the CONTENT PANE is previewing, or 0.
 *
 *  The data preview matches the stamp against its own `path` prop, because it
 *  also renders inside the file preview modal over a different file. A repo
 *  preview cannot: it is handed a parsed `RepoLocator`, never the encoded
 *  `repo:<id>:file:<path>` string the overlay holds and `refreshFilePreview`
 *  stamps. Re-encoding the locator to compare would be a round-trip that has to
 *  come back byte-identical, so the overlay answers instead. */
export const openFilePreviewRevision = computed(() => {
  const stamp = filePreviewRevision.value;
  return stamp && stamp.path === previewFile.value ? stamp.rev : 0;
});
export const panelTitle = signal<string | null>(null);
/** The URL the browser panel was opened at, so `webviewHasHistory` can tell
 *  whether the user has navigated inside the webview since. */
export const webviewInitialUrl = signal<string | null>(null);
export const fileSearchOpen = signal(false);
/** The toggle button that opened the file-search modal, passed to `<Overlay>`
 *  as the dismiss anchor, which the outside-pointerdown dismiss exempts. */
export const fileSearchAnchor = signal<HTMLElement | null>(null);

// --- Search Everywhere ---
export const searchEverywhereOpen = signal(false);

/** The toggle button that opened the modal. Passed to the dismiss hook as the
 *  anchor, so re-tapping the toggle closes via its own handler rather than
 *  letting the outside-pointerdown dismiss race the touch toggle. */
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
 *  projection of this set, and NO separately-fetched count exists to drift
 *  from it. Maintained by actions/notifications.ts: loaded on startup, resume
 *  and notification SSE, with optimistic removal on mark-read. Bounded,
 *  because unread is naturally small and the load is capped, so a pathological
 *  backlog renders as "99+". */
export const unreadNotifications = signal<Loadable<Notification[]>>({ status: 'not-loaded' });
/** Bell-badge count, DERIVED from the unread set and never independently
 *  fetched. The count IS the set's length, so the badge cannot contradict the
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
/** The browser tab's title: the unread count, the product name, and which
 *  workspace this is. Composed by `utils/windowTitle.ts`, which also composes
 *  the packaged window's own title from the same name. */
export const pageTitle = computed(() =>
  documentTitle(visibleWorkspaceName.value, unreadCount.value)
);
// --- Credentials ---
export const credentials = signal<Loadable<CredentialInfo[]>>({ status: 'not-loaded' });

// --- Environment variables (Settings → Environment Variables) ---
export const environmentVariables = signal<Loadable<EnvironmentVariable[]>>({ status: 'not-loaded' });

// --- Chat model registry (Settings → Models; drives the Lucidos Agent picker) ---
export const chatModels = signal<Loadable<ModelInfo[]>>({ status: 'not-loaded' });

// --- OAuth Accounts ---
export const oauthAccounts = signal<Loadable<OAuthAccountInfo[]>>({ status: 'not-loaded' });

/** The *OAuth provider registry*, as the engine serves it.
 *
 *  Drives the quick-provider buttons and the Connect form's autofill, so a
 *  provider added to `system-knowhow/oauth-providers.json` gets both with no
 *  frontend change. An engine with no staged system-knowhow answers an empty
 *  list, which renders no buttons and leaves the typed-name path working. */
export const knownOAuthProviders = signal<Loadable<KnownOAuthProviders>>({ status: 'not-loaded' });

/** What Settings → Accounts should arrive pre-filled with, set by a deep link.
 *
 *  Carries the SCOPES as well as the provider, because the caller knows what
 *  the connection is FOR. Backup passes its upload scopes, so one
 *  authorization covers sign-in and upload. Without them the user lands back
 *  on the Backup page facing *Grant access*, a second trip through the
 *  provider's consent screen for one intent.
 *
 *  Consumed once by `SettingsView` and cleared, like `settingsScrollTarget`. */
export const oauthConnectPrefill = signal<{ provider: string; scopes?: string } | null>(null);

// --- Triggers ---
export const triggers = signal<Loadable<TriggerInfo[]>>({ status: 'not-loaded' });

/** Thread Queue panel state: queued and running background spawns, plus the
 *  capacity policy. Refreshed on the queue and capacity-policy SSE events. */
export const threadQueue = signal<Loadable<ThreadQueueResponse>>({ status: 'not-loaded' });

export const historicalTriggers = signal<Loadable<HistoricalTriggerInfo[]>>({ status: 'not-loaded' });

/** User-visible folders that organize triggers in the panel. A pure label: a
 *  group fires and schedules nothing. Loaded from /trigger-groups on startup
 *  and kept live via SSE handlers in `thread-events.ts`. */
export const triggerGroups = signal<Loadable<TriggerGroup[]>>({ status: 'not-loaded' });

/** Per-device collapsed state for trigger-group sections, keyed by group_id.
 *  localStorage-backed, so a collapsed section stays collapsed across reloads
 *  and engine restarts on this device without syncing to any other. */
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

function persistCollapsedTriggerGroups(next: Set<string>): void {
  collapsedTriggerGroupIds.value = next;
  localStorage.setItem(COLLAPSED_TRIGGER_GROUPS_KEY, JSON.stringify([...next]));
}

export function toggleTriggerGroupCollapsed(groupId: string): void {
  const next = new Set(collapsedTriggerGroupIds.value);
  if (next.has(groupId)) next.delete(groupId);
  else next.add(groupId);
  persistCollapsedTriggerGroups(next);
}

/** Open a collapsed group, never close an open one. A deep link to a trigger
 *  inside a collapsed group has to expand it: `TriggersView` renders no members
 *  of a collapsed group, so the row's anchor does not exist to scroll to.
 *  Toggling instead would hide the row it was trying to reveal. */
export function expandTriggerGroup(groupId: string): void {
  if (!collapsedTriggerGroupIds.value.has(groupId)) return;
  const next = new Set(collapsedTriggerGroupIds.value);
  next.delete(groupId);
  persistCollapsedTriggerGroups(next);
}

/** A trigger id the Triggers panel should scroll to and mark once it renders.
 *  Set by `navigateToTrigger`, so a route to a trigger lands on its row rather
 *  than in the edit form. The row is where Run once, the pause toggle and the
 *  last-run status live. Mirrors `pluginScrollTarget`, and `TriggersView`'s
 *  effect consumes it once. */
export const triggerScrollTarget = signal<string | null>(null);

// --- Pending message (used to send a message from outside the chat module) ---
export const pendingChatMessage = signal<string | null>(null);

// Bumped when a coding-agent session starts or resumes, so components re-fetch
// their commands.
export const codingAgentSessionVersion = signal(0);

// Set from the compose view before a session starts, consumed on the first
// coding-agent message.
export const codingAgentPendingModel = signal<CodingAgentModelValue | null>(null);
export const codingAgentPendingReasoningEffort = signal<CodingAgentReasoningEffort | null>(null);

/** Reset the pending preferences. Called on thread switch and after sending. */
export function resetCodingAgentPendingPreferences(): void {
  codingAgentPendingModel.value = null;
  codingAgentPendingReasoningEffort.value = null;
}

// --- Apps ---
export const appsList = signal<Loadable<App[]>>({ status: 'not-loaded' });
export const marketplaceCatalog = signal<Loadable<MarketplaceCatalog>>({ status: 'not-loaded' });
/** Installed plugins for the Plugins → Installed tab, from GET
 *  /plugins/installed. That is the event projection rather than a marketplace
 *  scan, so it works offline and still lists a plugin whose marketplace was
 *  later removed. */
export const installedPlugins = signal<Loadable<InstalledPlugin[]>>({ status: 'not-loaded' });
/** Incremented to force-refresh app UI iframes, as a cache-busting key in the
 *  iframe src, so Preact propagates the reload to every iframe instance. 0 is
 *  the initial load, which needs no cache-buster. */
export const appRefreshKey = signal(0);
/** Incremented on every `AppUiRefreshRequested` frame: an app's files just
 *  changed on disk. `appRefreshKey` above reloads the running iframe. This one
 *  tells the app SOURCE editor to re-read what it shows, so an open editor
 *  cannot save a snapshot that predates an agent's edits. The editor freezes
 *  its epoch once the user types, because a draft outranks the disk
 *  (ADR 0118). */
export const appSourceEpoch = signal(0);
/** The WIP app preview. When set, the app UI iframe renders from the named app
 *  coding-agent thread's worktree instead of the live workspace data.
 *
 *  Three things clear it: a button re-click, navigating away from the thread
 *  or opening a different app (`actions/wipPreview.ts`), and a terminal change
 *  event (thread-sync.ts, plus `AppUiRefreshRequested` in refreshAppUI).
 *  Apply removes the worktree as part of the ff-merge, so cleanup must fire
 *  before the iframe re-renders. An iframe raises no `onError` for an HTTP
 *  4xx, so SSE-driven cleanup is the only reliable signal. */
export const wipPreviewThreadId = signal<string | null>(null);
export const pinnedApps = signal<Loadable<PinnedAppEntry[]>>(
  hydratePinnedAppsFromStorage(),
);

/** The **Plugins** panel's All / Installed filter. One catalog list shows
 *  installed and available plugins the same way, and this toggle narrows it.
 *  `false`, the default, is All: the whole catalog plus any installed plugin
 *  whose marketplace is gone. `true` is installed only. Persisted, so a reload
 *  returns to the same view. */
const PLUGINS_INSTALLED_ONLY_KEY = 'lucidos-plugins-installed-only';
export const pluginsInstalledOnly = signal<boolean>(
  localStorage.getItem(PLUGINS_INSTALLED_ONLY_KEY) === 'true',
);
export function setPluginsInstalledOnly(next: boolean): void {
  pluginsInstalledOnly.value = next;
  localStorage.setItem(PLUGINS_INSTALLED_ONLY_KEY, String(next));
}
/** A plugin id the Plugins panel's list should scroll to and pulse-highlight
 *  once it renders. The update-notification deep-link sets it, so a tap lands
 *  the user on the exact plugin with the pending update. Mirrors
 *  `settingsScrollTarget`, and the `StoreTab` scroll effect consumes it once. */
export const pluginScrollTarget = signal<string | null>(null);
/** The inline content-pane search bar in the Apps and Plugins panel headers.
 *  Filters the active list client-side, mirroring the thread search. Shared,
 *  because only one of those panels is visible at a time. */
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

/** The WRITABLE progress-dialog slot, for a dialog nothing else derives.
 *
 *  The two real flows do not write it: each is DERIVED from the signal that
 *  already says it is running (see `activeProgressDialog`). So the only writer
 *  left is the surface gallery, which fakes a run to show the shape. See
 *  docs/plans/2026-08-13-toast-banner-dialog-taxonomy.md. */
export const progressDialog = signal<ProgressDialogState>({
  visible: false,
  title: '',
  message: '',
  progress: null,
});

/** Whether the restart now in flight delivers a NEW engine version, which is
 *  the only thing the dialog's two shapes differ on. Set by
 *  `initiateEngineRestart` before it flips `engineRestarting`, and restored
 *  from the in-flight marker after a reload mid-restart. Read only while
 *  `engineRestarting` is true, so it needs no clearing. */
export const engineRestartNewVersion = signal(false);

/** The progress dialog on screen, or an invisible state when there is none.
 *
 *  DERIVED, and that is the whole point: a modal written by hand at each site
 *  strands the user at the first site that forgets to clear it. Each flow
 *  already owns a signal that says whether it is running, so the dialog rides
 *  that signal and every existing clear closes it. Reconnect, the restart
 *  safety timeout, a spawn failure and each terminal update frame all cost
 *  nothing here.
 *
 *  The restart wins a tie. The two cannot genuinely overlap, since a packaged
 *  install restarts the whole stack rather than the engine. A precedence makes
 *  that overlap unrepresentable instead of racy. */
export const activeProgressDialog = computed<ProgressDialogState>(() => {
  if (engineRestarting.value) return restartDialogState(engineRestartNewVersion.value);
  const frame = appUpdateProgress.value;
  if (frame) return appUpdateDialogState(frame, () => { void cancelAppUpdate(); });
  return progressDialog.value;
});

// --- Prompt dialog ---
export const promptState = signal<PromptState>({
  visible: false,
  message: '',
});

// --- File preview modal ---
/** A file an app asked the host to show over it, without navigating the shell
 *  away (`lucidos.ui.previewFile`). Read-only, and deliberately NOT a
 *  `panelOverlay` variant: that is the content pane's nav-history unit, and a
 *  glance at a cited file is not a destination Back should walk onto.
 *
 *  Opened and closed through `store/actions/filePreviewModal`, which owns the
 *  view-state borrowing that makes the shared preview components render it. */
export interface FilePreviewModalState {
  /** Bumped per open, so a second `previewFile` that replaces a showing modal
   *  re-runs the component's per-open effects instead of looking unchanged. */
  id: number;
  /** The resolved locator: a workspace data path, or a `repo:` encoded path.
   *  The renderer parses it with `parseRepoPath`, exactly as `ContentPane`
   *  parses the panel's own file-preview path. */
  path: string;
  /** The line range the modal opened at, or null when the citation named none.
   *  Kept so the escalation into the Files panel carries the same lines. */
  range: { start: number; end: number } | null;
}
export const filePreviewModal = signal<FilePreviewModalState | null>(null);

// --- Toasts ---
let toastIdCounter = 0;
export const toasts = signal<ToastItem[]>([]);
/** Standard "passive status banner" duration. A keyed toast defaults to sticky
 *  (see `scheduleAutoDismiss`). A caller with no action button to wait on opts
 *  back in here, so every such banner shares one tunable. */
export const TOAST_AUTO_DISMISS_MS = 5_000;
/** Pending auto-dismiss timers for keyed toasts. Cleared when the same key is
 *  re-shown, restarting the window, or when the toast is dismissed some other
 *  way. Without the cleanup the Map entry survives until the timeout fires. */
const keyedDismissTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** Is the workspace unable to serve requests right now, for a reason that is
 *  already stated on screen by one authoritative toast?
 *
 *  Three ways in, all the same situation reached differently: every request in
 *  flight fails at once, for one cause, and a per-request failure toast adds
 *  nothing over the status toast already up.
 *
 *    • `engineRestarting`: the engine goes down under us, with the
 *      UiBlockingOverlay covering the screen and a status toast narrating it.
 *    • `appUpdateCommitted`: a packaged update past its point of no return
 *      restarts the launchd service, killing the gateway serving this page.
 *    • `!databaseReachable`: the engine is up and answering `/health`, but its
 *      database is not, so every query behind it fails. This one lasts as long
 *      as Docker is down, which is why one accurate toast beats twenty
 *      inaccurate ones.
 *
 *  Suppression is legitimate only WITH that authoritative toast, which opts in
 *  via `showWhileUnavailable`. Each producer above owns one. */
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
  // Freeze the toast over the pane focused right now, so a later focus switch
  // cannot make it jump panes. The drawer counts as the thread pane. The
  // keyed-update branch above deliberately does NOT touch `pane`, so an
  // in-place update keeps the toast where it first appeared.
  const pane = focusedPane.value === 'content' ? 'content' : 'thread';
  // Prepend, so the newest toast renders at the top of its pane's column and
  // pushes that pane's existing toasts down. Each column is pinned to the top
  // of the viewport, so array order runs top to bottom.
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

/** Structurally remove a keyed toast WITHOUT the user-dismiss side effects, so
 *  it clears no badge and records no build as user-dismissed. Use it to keep a
 *  toast in lockstep with the signal driving it. A signal-driven hide must
 *  never be mistaken for the user dismissing the prompt, which would suppress
 *  it for that build. `dismissToast` is the user path and adds the side
 *  effects on top. */
export function removeToast(key: string): void {
  // Only REASSIGN when that key is actually showing. A fresh array notifies
  // every subscriber even when it removed nothing. A caller keeping a toast in
  // lockstep with a signal makes exactly that no-op call on its happy path.
  // Timer cleanup below stays unconditional.
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
  // The user-dismiss side effect, per update surface: remember THIS build as
  // dismissed (keyed by build id in hooks/sw-update.ts) so the honest
  // re-checks do not re-surface the toast. A genuinely newer build still will.
  //
  // Dismiss means "defer to later", so it does NOT clear the badge. The badge
  // stays lit as the persistent update affordance, and the sync and poll
  // checks re-derive it from staleness, so it clears on its own.
  if (idOrKey === 'update-available') {
    markSwUpdateDismissed();
  } else if (idOrKey === NEW_VERSION_TOAST_KEY) {
    // One key, both shapes the engine-version toast takes (a built version to
    // switch onto, and one that exists only in source). The poll records which
    // one is being announced; this only has to spend it. See
    // `noteAnnouncedEngineVersion`.
    markEngineVersionDismissed();
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
    acknowledge?: boolean;
  }
): Promise<boolean> {
  // A second call replaces, never queues: resolve any visible confirm's
  // Promise as `false` before showing the new one.
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
      acknowledge: options?.acknowledge,
    };
  });
}

/** Show a text-input modal. Resolves the entered string on OK, or `null` on
 *  Cancel, Escape or a backdrop click. Like {@link showConfirm}, a second call
 *  replaces a visible prompt and never queues, and the prior resolves `null`. */
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

// --- Command checkpoint diff modal ---
// Set to a checkpoint ResponseEvent to show what that command changed; null =
// closed. Opened by the Diff button on the checkpoint card, so the Undo beside
// it is not a blind button. A modal rather than an inline expansion, because a
// destructive command can touch a lot of files.
export type CheckpointDiffModalState = Extract<ResponseEvent, { type: 'checkpoint' }> | null;
export const checkpointDiffModal = signal<CheckpointDiffModalState>(null);

// --- Context viewer ---
// Set to a captured context to open the viewer; null = closed. Its only door
// is the context counter on a step row, since the rest of the row opens the
// step detail. It holds the SNAPSHOT rather than the step, so the viewer
// cannot be opened for a row that has none. `description` rides along purely
// as the subtitle naming which call the context belongs to.
export interface ContextViewerState {
  snapshot: ContextCapture;
  description?: string;
}
export const contextViewer = signal<ContextViewerState | null>(null);

// --- Event subscription condition ---
// Set to one *event subscription* to show the `condition` filtering it; null =
// closed. One subscription rather than the whole `on:` list, because that is
// what the thing you pressed names.
export interface EventConditionModalState {
  eventType: string;
  condition: Record<string, unknown>;
}
export const eventConditionModal = signal<EventConditionModalState | null>(null);

/** The door to a subscription's condition, or `null` when it has none.
 *
 *  **Both PRESSABLE surfaces go through here**: the transcript row's chip and
 *  the waiting panel's line. (The archive confirmation prints the same
 *  "(filtered)" label into a plain string and offers no door.) They must open the same thing under
 *  the same accessible name, and each must be pressable exactly when its label
 *  says filtered. Two copies of that rule is how the two drift apart.
 *
 *  The panel is a backdrop-less popover and the modal is a top-anchored sheet.
 *  So opening one from the other STACKS on `overlayStack` rather than replacing
 *  it: Escape or an outside click closes the modal and lands back on the panel.
 *
 *  `condition` is captured here rather than re-read at click time, which is
 *  what makes the returned `open` safe to hand to a handler. */
export function eventConditionDoor(
  s: EventSubscription,
): { label: string; open: () => void } | null {
  const condition = s.condition;
  if (!condition) return null;
  return {
    label: `Show the condition filtering ${s.event_type}`,
    open: () => {
      eventConditionModal.value = { eventType: s.event_type, condition };
    },
  };
}

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

/** Bumped when a `PreferencesChanged` carries one of the three keys the Backup
 *  page renders: `backup_provider`, `backup_schedule`, `backup_retention`.
 *  BackupSection re-seeds its provider, schedule and retention controls on it.
 *
 *  A SIBLING of `backupStatusVersion` rather than a widening of it, on
 *  purpose. That signal means "a backup RUN reached a terminal state", and the
 *  health card refetches `/backup/status` on it. Making a retention or
 *  schedule write bump the same counter would put a cloud round-trip behind a
 *  local preference edit. Two meanings, two signals.
 *
 *  Without either, the panel reads its three values once on mount and nothing
 *  moves them. An agent writing `backup_provider`, or the same page open on a
 *  second device, then leaves the dropdown showing the old destination. */
export const backupPreferencesVersion = signal(0);

/** Bumped on every `McpServerRegistered` / `McpServerUpdated` /
 *  `McpServerRemoved` / `McpServerDisabledToolsChanged` frame. Settings → MCP
 *  Servers re-reads `/mcp/servers` on it. A server the agent registers, or a
 *  tool switched off on another device, then reaches the open page
 *  (ADR 0118). */
export const mcpServersVersion = signal(0);

/** Bumped on every `WebhookCreated` / `WebhookUpdated` / `WebhookDeleted`
 *  frame. Settings → Webhooks re-reads its list on it. Hooks are created and
 *  disabled from the CLI as often as from this page, so the open page was
 *  otherwise stale for the whole session. */
export const webhooksVersion = signal(0);

/** Whether the public path a webhook delivery arrives on is reachable, per
 *  address family.
 *
 *  Read by two surfaces: the app bar that shows while a family is down, and
 *  every enabled row on the Webhooks page. Loaded at startup, because the bar
 *  has to be able to raise itself, and rewritten by the two `WebhookIngress*`
 *  frames while the app is open.
 *
 *  Nothing here is this client's own connectivity. `connectionStatus` says
 *  whether the engine answers THIS browser. An ingress outage is the opposite
 *  case: the app is fine and the machine cannot be reached from outside. */
export const webhookIngress = signal<Loadable<IngressReading>>({ status: 'not-loaded' });

/** Bumped on every `PermissionGrantsChanged` frame. The allowlist editors in
 *  Settings → Permissions re-read their file on it, unless the user has
 *  unsaved patterns in the editor: a draft outranks the disk (ADR 0118).
 *
 *  Not keyed by grant file. Both files are small and both are on the page
 *  already, so re-reading the other one costs one request. Keying it would
 *  put a routing decision in the arm, which is one more thing to keep in
 *  sync. */
export const permissionGrantsVersion = signal(0);

/** Bumped on every `HandshakeScriptApproved` frame. The file preview re-reads
 *  which auth handshake scripts may run. So the warning on an unapproved one
 *  clears the moment an approval lands, on every open client (ADR 0118). */
export const handshakeScriptsVersion = signal(0);

/** Updated from SSE RecoveryProgress events. null = not recovering. */
export const recoveryProgress = signal<{ completed: number; total: number } | null>(null);

// --- Update available ---
export const updateAvailable = signal(false);

// --- New engine version ready to switch onto (dev background rebuild) ---
// Set by the version-status poll (store/actions/engine-update.ts) when a newer
// engine binary is on disk. Drives the Switch toast and the brand badge.
// Distinct from `updateAvailable`, the client-bundle refresh, and from
// `restartRequired`, which says a restart-requiring change was applied. This
// is the honest "the rebuilt engine is READY to switch to" signal.
export const engineVersionReady = signal(false);

// --- New engine version pending in SOURCE, with nothing built to switch onto ---
// Set by the version-status poll (store/actions/engine-update.ts) when the
// engine source is behind HEAD, no newer binary is on disk, and nothing is
// building one. The third state of the brand badge (a dot, not the ready "!"),
// and the driver of the pending version toast.
//
// Distinct from `engineVersionReady` in the way that matters to the user. THAT
// one says "there is something you can switch onto right now". This one says
// "there is new code, and it has not become a version yet". Offering a Switch
// here would respawn the same engine.
export const engineVersionPending = signal(false);

// --- ...and rebuilding has been proved unable to deliver it ---
// The engine's verdict (`rebuild_wedged`), never derived here. Only the engine
// knows which HEAD a completed build was built from, and a build that finished
// before newer commits landed says nothing about a rebuild now. Meaningful
// only while `engineVersionPending`. It tints the badge and swaps the toast's
// Rebuild button for the operator fix.
export const engineRebuildWedged = signal(false);

// --- New engine version currently building (dev background rebuild) ---
// Set by the version-status poll (store/actions/engine-update.ts) when the
// engine reports `build_state === 'building'`: a background rebuild is in
// progress but not yet ready to switch onto. Drives the spinning-refresh brand
// badge, and is always false in a packaged build.
//
// What the toast can say ABOUT that build rides `engineBuildDetail`, in
// `store/backgroundActivity.ts`. One writer, `setEngineBuilding`, sets both,
// so the boolean and the narration cannot drift apart.
export const engineBuilding = signal(false);

/** Whether a new engine version is READY to switch onto. Lives here rather
 *  than in a component so the brand badge AND the restart progress-toast
 *  wording derive from the SAME predicate, with no import cycle. Same
 *  reasoning as `NEW_VERSION_TOAST_KEY` below.
 *
 *  In **dev** this is `engineVersionReady` alone. Apply is non-disruptive and
 *  kicks off a background rebuild, so a freshly-applied restart-requiring
 *  change does NOT mean a new version exists yet. The switch becomes available
 *  once that build finishes and the on-disk binary differs. Restarting during
 *  the build window respawns the OLD binary, so keying off `restartRequired`
 *  here would falsely claim a new version.
 *
 *  In **packaged** there is no background build, and a newer GitHub release is
 *  immediately installable, so `restartRequired` IS the ready signal.
 *  `engineVersionReady` never fires there, since the poll no-ops.
 *
 *  `restartRequired` deliberately still gates the client-refresh ordering,
 *  which is a different concern from this visible signal. */
export function engineNewVersionReady(): boolean {
  return engineVersionReady.value || (enginePackaged.value && restartRequired.value);
}

// Toast key for the poll-driven Switch info toast
// (store/actions/engine-update.ts). Lives here rather than there so
// `initiateEngineRestart` can dismiss it when a switch begins, with no import
// cycle. The progress dialog then replaces it as the version surface.
export const NEW_VERSION_TOAST_KEY = 'engine-new-version';

// Toast key for a failed thread-list refresh. Both surfacing sites share it,
// so a sustained outage replaces one toast rather than stacking a new one on
// every SSE reconnect. Neither reads the key directly: both go through
// `refreshThreadList` (store/actions/thread-list-refresh.ts), the only writer,
// so the copy, the raising rule and the retraction cannot drift.
export const THREAD_LIST_REFRESH_TOAST_KEY = 'thread-list-refresh-failed';

// Toast keys for the two per-thread event fetches (thread-loading.ts).
//
// The LOAD one fans out, one full snapshot per eagerly-loaded thread on boot
// and per failed thread on the recovery path. Unkeyed, one outage means one
// permanent, undismissable toast PER THREAD. Keyed, the whole fan-out
// collapses into one card whose copy counts the affected threads.
//
// The REFRESH one no longer fans out, since a sync point marks instead. It
// keeps its key because several threads can still be failing at once as the
// user moves between them.
//
// Two keys, not one. A LOAD failure means this device never got the thread's
// history; a REFRESH failure means it did not get the newest events. Neither
// card may retract the other while its own claim is true.
export const THREAD_EVENTS_LOAD_TOAST_KEY = 'thread-events-load-failed';
export const THREAD_EVENTS_REFRESH_TOAST_KEY = 'thread-events-refresh-failed';

/** How many per-thread event fetches one fan-out may have in flight at once.
 *  Two fan-outs remain, both full snapshot loads: the eager boot loads in
 *  `loadAllThreads`, and the failed-load retry in `runResumeSync`.
 *
 *  Over HTTP/2 the browser applies no per-host connection cap, so an unbounded
 *  fan-out over a large workspace puts ~85 requests a minute onto one
 *  connection, all racing the same 10s client deadline. The engine answers
 *  each in single-digit milliseconds, so the burst itself is what spends those
 *  deadlines. Four keeps the link saturated without the herd.
 *
 *  Deliberately PER FAN-OUT, not a global semaphore. A recovery wake can run
 *  both concurrently, so the real ceiling there is eight. A global cap would
 *  buy little and cost the property that matters most on a wake: the focused
 *  thread's fetch would queue behind unrelated background work.
 *
 *  Lives here rather than in `thread-loading.ts` for the same reason the toast
 *  keys above do. Its other consumer is an action module that would otherwise
 *  import it across a mocked boundary. */
export const THREAD_EVENTS_FETCH_CONCURRENCY = 4;

/** How often the connection watchdog probes `/api/v1/health`. This poll alone
 *  drives the dot the user sees. So this number and the deadline below are the
 *  whole timing contract of that dot, and are only meaningful against each
 *  other. They live here together for that reason, and because their consumers
 *  would otherwise each own half of a relation neither can see. */
export const CONNECTION_POLL_INTERVAL_MS = 5000;

/** Deadline for one health probe. Must stay STRICTLY BELOW
 *  `CONNECTION_POLL_INTERVAL_MS`, which is what keeps at most one probe in
 *  flight from the timer. Overlapping probes would queue on the same HTTP/2
 *  connection and time out in turn, manufacturing the outage the dot reports.
 *  `connection-poll-budget.test.ts` pins the relation.
 *
 *  Sized for a phone reaching a laptop over cellular and a Tailscale tunnel: a
 *  radio state transition alone can spend seconds, and a DERP relay hop adds
 *  more. Four consecutive misses paint the dot red, so a merely tight deadline
 *  reads as an outage. A generous one costs no extra requests, and only a
 *  HANGING request is affected: a genuinely dead engine refuses the connection
 *  and fails fast whatever the deadline says. */
export const HEALTH_PROBE_TIMEOUT_MS = 4500;

// Toast key for the "takes effect on Switch" hint, shown when the engine emits
// FrontendUpdateDeferred: a frontend-only Apply could not advance the served
// client in-process, because an engine version change is pending. See
// engine::frontend_refresh INV-A. Keyed, so repeated frontend-only applies
// while a Switch is pending coalesce into one toast. Lives here so
// initiateEngineRestart can drop it when the switch begins with no import
// cycle, the same pattern as NEW_VERSION_TOAST_KEY.
export const FRONTEND_UPDATE_DEFERRED_TOAST_KEY = 'engine-frontend-update-deferred';

// Sibling of the key above for the STRANDED case: the frontend change rebuilt,
// but the engine serves a dist/ that will never receive it, so no Switch
// delivers it. A separate key, so a stranded warning cannot be coalesced into
// the "arrives on Switch" hint, which would be actively misleading.
export const FRONTEND_UPDATE_STRANDED_TOAST_KEY = 'engine-frontend-update-stranded';

// --- Service worker build id ---
/** BUILD_ID of the active service worker, stamped into sw.js by the
 *  `lucidos-sw-stamp` Vite plugin. The SW reports it on request and the
 *  control panel shows it. A debugging aid for "did the new build's SW take
 *  over?". It is the same value whose byte-change fires the update toast, so
 *  an unchanged id across an apply means the SW never updated. `null` until
 *  the SW answers, and the live dev server reports the un-stamped
 *  placeholder. */
export const serviceWorkerBuildId = signal<string | null>(null);
