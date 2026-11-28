import { PREF_CODING_AGENT_DEFAULT, PREF_NOTIFICATIONS_FILTER } from '@lucidos/preference-catalog';
import { readWindowItem, removeWindowItem, writeWindowItem } from '../utils/windowStorage';
import { signal, computed, batch, type ReadonlySignal, type Signal } from '@preact/signals';
import type { WidgetParams } from '../utils/widgetParams';
import { hydratePinnedAppsFromStorage } from './actions/pinnedApps';
import { gapClearedProgress, gapRefreshedVersion } from './eventGap';
import { enginePackaged, restartRestartsGateway } from './enginePackaging';
import { WORKSPACE_ID } from '../utils/basePath';
import { minDrawerWidth } from './paneMinimums';
import { armRestoreGuard, readGuardedFocus } from './restoreGuard';
import { DRAWER_GROUPING_KEY, SELECTED_ONGOING_GROUP_KEY, THREAD_DRAWER_WIDTH_KEY } from './drawerKeys';
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
  ConfirmAlternative,
  ConfirmState,
  ConfirmDetails,
  ConfirmVariant,
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
  RefusalReading,
} from './types';
import { MENU_ITEMS } from './types';
import type { AppUpdateRunning } from '../utils/tauri';
import { cancelAppUpdate } from '../utils/tauri';
import { documentTitle } from '../utils/windowTitle';
import { errorDetail } from '../utils/errorDetail';
import { holdSoftwareKeyboard } from '../utils/softwareKeyboard';
import { restartDialogState, appUpdateDialogState, restartTakesPageDown } from './progressDialogCopy';
import type { ConnectionNoticeStatus } from '../utils/connectionNotice';
import { clampToastText } from '../components/shared/toastMessage';
import type { SubscriptionGroup, ThreadMeta, ThreadState, ThreadStatus, Exchange } from './thread-events';
import { changeReadyToReview, computeExchanges, isExcludedFromSections, plainEventName } from './thread-events';
import { getThreadEventsBump } from './threadActivity';
import { DEFAULT_CHAT_MODEL } from './models';
import { type PreferenceValues } from '@lucidos/preference-catalog';
import { awaitsOwnerAnswer, displaySection, EVENT_CHANNELS } from '../generated/thread-lifecycle';
import type { EventChannel, ArchiveState, DisplaySection } from '../generated/thread-lifecycle';
import type { UnproposedReason } from '../generated/thread-event-wire';
import { resetContentScroll } from './savedScroll';
import type { Change, ChangelogRelease, DeferredBuild, PendingCommits, ReaderDevices, ReleaseNoticeView, ThreadContext } from '../api/client';
import type { UpdateRelay, ReleaseCheck } from '../api/client/control';
import type { LastSuccessfulBackupResponse } from '../api/client/settings';
import type { ApplyEstimates, LandingLane, MainCheck } from '../api/client/changes';
import type { EnvironmentVariable, IntelligenceLadder, ModelInfo, ResponseStyle } from '../api/types';
import type { ChunkGate } from '../utils/lazyComponent';
import { markSwUpdateDismissed, markEngineVersionDismissed, markBuildFailureDismissed } from '../hooks/sw-update';
import { parseRepoPath } from './repoPath';
import { archivePending } from './rowIntents';
import { withoutPendingReads, withPendingReadsShown, type PendingReads } from './notificationReads';

export const FOCUSED_THREAD_KEY = 'lucidos-focused-thread';

export type InlineForm =
  | { type: 'credential'; editing?: string; request?: CredentialRequest }
  | { type: 'app-edit'; appId: string }
  | { type: 'new-app' }
  | { type: 'trigger'; triggerId?: string }
  | { type: 'email-confirm'; request: EmailConfirmRequest; sentAt?: string }
  | { type: 'plugin-install'; request: PluginInstallRequest; installed?: PluginInstallReceipt }
  | { type: 'plugin-uninstall'; request: PluginUninstallRequest; removed?: PluginUninstallReceipt };

export type EmailConfirmForm = Extract<InlineForm, { type: 'email-confirm' }>;

export type PluginInstallForm = Extract<InlineForm, { type: 'plugin-install' }>;

export type PluginUninstallForm = Extract<InlineForm, { type: 'plugin-uninstall' }>;

export type PanelOverlay =
  | { type: 'form'; form: InlineForm }
  | { type: 'app-ui'; app: App; fragment?: string; params?: WidgetParams }
  | { type: 'file-preview'; path: string }
  | { type: 'url-preview'; url: string }
  | { type: 'notification-detail'; notification: Notification }
  | { type: 'plugin-detail'; marketplaceId: string; pluginId: string; name: string }
  | { type: 'terminal'; threadId: string }
  | null;

export const panelOverlay = signal<PanelOverlay>(null);

export const activeInlineForm = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'form' ? o.form : null;
});
export const currentApp = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'app-ui' ? o.app : null;
});
export const currentAppParams = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'app-ui' ? o.params : undefined;
});
export const currentAppFragment = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'app-ui' ? o.fragment ?? null : null;
});
export const previewFile = computed(() => {
  const o = panelOverlay.value;
  return o?.type === 'file-preview' ? o.path : null;
});

export const filePreviewSource = signal(localStorage.getItem('lucidos-file-preview-source') === 'true');

export const filePreviewWrap = signal(localStorage.getItem('lucidos-file-preview-wrap') !== 'false');

export const diffWholeFile = signal<boolean | null>(null);

export const diffSideBySide = signal(localStorage.getItem('lucidos-diff-side-by-side') === 'true');

export const diffFitsSideBySide = signal(true);

export const filePreviewEditing = signal(false);

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

export const notificationDetailPending = signal<string | null>(null);

export function closeInlineForm(): void {
  const form = activeInlineForm.value;
  if (form?.type === 'trigger') resetContentScroll('triggers');
  panelOverlay.value = null;
}

export function closeInlineFormIfActive(form: InlineForm): void {
  if (activeInlineForm.value === form) closeInlineForm();
}

export type SettingsSubview = 'main' | 'system' | 'system-overview' | 'providers' | 'models' | 'intelligence' | 'appearance' | 'memory' | 'devices' | 'users' | 'accounts' | 'backup' | 'coding-agents' | 'locale' | 'marketplaces' | 'disk-usage' | 'prompt-footprint' | 'permissions' | 'mcp' | 'keyboard-shortcuts' | 'access' | 'webhooks' | 'event-hooks' | 'environment-variables' | 'thread-queue' | 'whats-new' | 'release-notices' | 'debugging' | 'developer' | 'communication-surfaces' | 'updates';
export type SettingsNavKey = Exclude<SettingsSubview, 'main'>;
export interface SettingsNavItem {
  key: SettingsNavKey;
  label: string;
  short?: string;
}
export type SettingsNavGroup = 'Assistant' | 'Workspace' | 'Gateway' | 'This device';
export interface SettingsHomeNavItem extends SettingsNavItem {
  group: SettingsNavGroup;
}
export const settingsSubview = signal<SettingsSubview>('main');
export const settingsScrollTarget = signal<string | null>(null);
export const settingsLinkedFragment = signal<string | null>(null);

export const SETTINGS_SYSTEM_SUBPANEL_ITEMS: SettingsNavItem[] = [
  { key: 'system-overview', label: 'Overview' },
  { key: 'release-notices', label: 'Release Notices', short: 'Notices' },
  { key: 'whats-new', label: "What's New" },
  { key: 'thread-queue', label: 'Thread Queue' },
  { key: 'backup', label: 'Backup' },
  { key: 'memory', label: 'Memory' },
  { key: 'disk-usage', label: 'Disk Usage' },
  { key: 'prompt-footprint', label: 'Workspace Prompt Footprint', short: 'Footprint' },
  { key: 'environment-variables', label: 'Environment Variables', short: 'Env Vars' },
  { key: 'debugging', label: 'Debugging' },
  { key: 'developer', label: 'Developer' },
  { key: 'communication-surfaces', label: 'Communication Surfaces', short: 'Surfaces' },
];

export const SETTINGS_NAV_ITEMS: SettingsHomeNavItem[] = [
  { key: 'providers', label: 'Providers', group: 'Assistant' },
  { key: 'models', label: 'Models', group: 'Assistant' },
  { key: 'intelligence', label: 'Intelligence', group: 'Assistant' },
  { key: 'permissions', label: 'Permissions', group: 'Assistant' },
  { key: 'mcp', label: 'MCP Servers', group: 'Assistant' },
  { key: 'coding-agents', label: 'Coding Agents', short: 'Agents', group: 'Assistant' },
  { key: 'accounts', label: 'Accounts', group: 'Workspace' },
  { key: 'locale', label: 'Locale', group: 'Workspace' },
  { key: 'marketplaces', label: 'Marketplaces', group: 'Workspace' },
  { key: 'webhooks', label: 'Webhooks', group: 'Workspace' },
  { key: 'event-hooks', label: 'Event Hooks', group: 'Workspace' },
  { key: 'system', label: 'System', group: 'Workspace' },
  { key: 'access', label: 'Access', group: 'Gateway' },
  { key: 'devices', label: 'Devices', group: 'Gateway' },
  { key: 'users', label: 'Users', group: 'Gateway' },
  { key: 'updates', label: 'Updates & Installs', short: 'Updates', group: 'Gateway' },
  { key: 'appearance', label: 'Appearance & Behavior', short: 'Appearance', group: 'This device' },
  { key: 'keyboard-shortcuts', label: 'Keyboard Shortcuts', short: 'Shortcuts', group: 'This device' },
];

export const SETTINGS_SUBVIEW_ITEMS: SettingsNavItem[] = [
  ...SETTINGS_NAV_ITEMS,
  ...SETTINGS_SYSTEM_SUBPANEL_ITEMS,
];

function settingsNavItem(key: Exclude<SettingsSubview, 'main'>): SettingsNavItem | undefined {
  return SETTINGS_SUBVIEW_ITEMS.find(item => item.key === key);
}

export function settingsSubviewLabel(key: Exclude<SettingsSubview, 'main'>): string | undefined {
  return settingsNavItem(key)?.label;
}

export function settingsSubviewShortLabel(key: Exclude<SettingsSubview, 'main'>): string | undefined {
  const item = settingsNavItem(key);
  return item && (item.short ?? item.label);
}

const RETIRED_SETTINGS_SUBVIEWS = new Map<string, SettingsSubview>([
  ['links', 'appearance'],
  ['experimental', 'appearance'],
  ['repositories', 'coding-agents'],
  ['mobile-access', 'access'],
  ['network-access', 'access'],
  ['themes', 'appearance'],
]);

export function migrateSettingsSubview(raw: unknown): SettingsSubview {
  if (typeof raw !== 'string') return 'main';
  if (raw === 'main') return 'main';
  const moved = aliasRetiredSettingsSubview(raw);
  const live = SETTINGS_SUBVIEW_ITEMS.some(i => i.key === moved);
  return live ? (moved as SettingsSubview) : 'main';
}

export function aliasRetiredSettingsSubview(raw: string): string {
  return RETIRED_SETTINGS_SUBVIEWS.get(raw) ?? raw;
}

interface MovedSettingsAnchor {
  anchor: string;
  view: Exclude<SettingsSubview, 'main'>;
}

const RETIRED_SETTINGS_ANCHORS = new Map<string, MovedSettingsAnchor>([
  ['models:providers', { anchor: 'providers', view: 'providers' }],
  ['models:vertex-ai', { anchor: 'providers:vertex', view: 'providers' }],
  ['models:region', { anchor: 'providers:vertex-region', view: 'providers' }],
  ['command-safety:judge-model', { anchor: 'models:command-judge', view: 'models' }],
  ['command-safety:coding-agents', { anchor: 'permissions:coding-agent-auto-answer', view: 'permissions' }],
  ['appearance:agent-brand-colours', { anchor: 'appearance:agent-icon-colors', view: 'appearance' }],
  ['system:update-check', { anchor: 'updates:update-check', view: 'updates' }],
  ['system:update-channel', { anchor: 'updates:update-channel', view: 'updates' }],
  ['system:installs', { anchor: 'updates:installs', view: 'updates' }],
  ['access:network', { anchor: 'access:machine', view: 'access' }],
  ['system:engine-bind', { anchor: 'access:machine', view: 'access' }],
  ...[
    'anthropic', 'openai', 'openrouter', 'xai', 'opencode-free', 'local', 'claude-gateway',
    'typesafe', 'cloudflare-workers-ai', 'system-one-custom',
  ].flatMap((id): [string, MovedSettingsAnchor][] => [
    [`models:${id}`, { anchor: `providers:${id}`, view: 'providers' }],
    [`models:${id}-usage`, { anchor: `providers:${id}-usage`, view: 'providers' }],
  ]),
]);

export const RETIRED_SETTINGS_ANCHOR_IDS: readonly string[] = [...RETIRED_SETTINGS_ANCHORS.keys()];

export function aliasRetiredSettingsAnchor(raw: string): string {
  return RETIRED_SETTINGS_ANCHORS.get(raw)?.anchor ?? raw;
}

export function settingsLinkView(view: string, fragment: string): string {
  return RETIRED_SETTINGS_ANCHORS.get(fragment)?.view ?? view;
}

{
  const savedMenu = readWindowItem('lucidos-active-menu-item');
  const legacyAppsTab = localStorage.getItem('lucidos-apps-tab');
  if (savedMenu === 'app-store' || (savedMenu === 'apps' && legacyAppsTab === 'store')) {
    writeWindowItem('lucidos-active-menu-item', 'plugins');
  }
  localStorage.removeItem('lucidos-apps-tab');
  localStorage.removeItem('lucidos-plugins-tab');
}
const savedMenuItem = readWindowItem('lucidos-active-menu-item');
export const activeMenuItem = signal<MenuItem>(
  savedMenuItem && (MENU_ITEMS as readonly string[]).includes(savedMenuItem)
    ? (savedMenuItem as MenuItem)
    : 'files'
);

export const connectionStatus = signal<ConnectionStatus>('connecting');
export const isConnected = computed(() => connectionStatus.value === 'connected');
export const workspaceName = signal<string>('');
export const workspaceDisplayName = signal<string>('');
export const visibleWorkspaceName = computed(
  () => workspaceDisplayName.value || workspaceName.value,
);
export const workspacePath = signal<string>('');
export const engineStartedAt = signal<string | null>(null);
export const lucidosRelease = signal<string | null>(null);
export const lucidosReleaseDirty = signal<boolean>(false);
export const changelogReleases = signal<Loadable<ChangelogRelease[]>>({ status: 'not-loaded' });
export const whatsNewSeenRelease = signal<string | null>(
  localStorage.getItem('lucidos-whats-new-seen-release'),
);
export const whatsNewTargetRelease = signal<string | null>(null);
export const releaseNoticeView = signal<Loadable<ReleaseNoticeView>>({ status: 'not-loaded' });
export const readerDevices = signal<Loadable<ReaderDevices>>({ status: 'not-loaded' });
export const engineVersion = signal<string | null>(null);
export const latestEngineVersion = signal<string | null>(null);
export const latestTauriAppVersion = signal<string | null>(null);
export const clientOfferedRelease = signal<string | null>(null);
export const latestTauriAppNotes = signal<string | null>(null);
export const releaseCheck = signal<ReleaseCheck | null>(null);
export const updateRelay = signal<UpdateRelay | null>(null);
export interface RelayedUpdate {
  id: string;
  version: string;
  fromVersion: string;
  since: number;
}
export const relayedUpdate = signal<RelayedUpdate | null>(null);
export const appUpdateCheckError = signal<string | null>(null);
export const appUpdateCheckInFlight = signal(false);
export const appUpdateProgress = signal<AppUpdateRunning | null>(null);
export const appUpdateCommitted = computed(() => {
  const phase = appUpdateProgress.value?.phase;
  return phase === 'installing' || phase === 'restarting-services' || phase === 'relaunching';
});
export const databaseReachable = signal(true);
export { enginePackaged };

export const llmConfigured = signal<boolean>(true);

export const agentSandboxUnavailable = signal<string | null>(null);

export const agentSandboxTruncationUnguarded = signal<boolean>(false);

export const agentSandboxMetadataUnguarded = signal<boolean>(false);

export const configuredProviders = signal<string[] | null>(null);

export const currentModel = signal(DEFAULT_CHAT_MODEL);


export { animationSpeed, speedMultiplier, durationScale, scaledDurationMs } from '../utils/motion';

export const threadDrawerOpen = signal(
  localStorage.getItem('lucidos-thread-drawer-open') === 'true'
);
export type DrawerGrouping = 'folders' | 'ongoing';

function restoreDrawerGrouping(): DrawerGrouping {
  return localStorage.getItem(DRAWER_GROUPING_KEY) === 'ongoing' ? 'ongoing' : 'folders';
}

const drawerGroupingState = signal<DrawerGrouping>(restoreDrawerGrouping());

export const drawerGrouping: ReadonlySignal<DrawerGrouping> = drawerGroupingState;

export function setDrawerGrouping(grouping: DrawerGrouping): void {
  drawerGroupingState.value = grouping;
  if (grouping === 'folders') localStorage.removeItem(DRAWER_GROUPING_KEY);
  else localStorage.setItem(DRAWER_GROUPING_KEY, grouping);
}

export const ONGOING_GROUPS = ['blocked', 'review', 'drafts', 'in-flight'] as const;
export type OngoingGroup = typeof ONGOING_GROUPS[number];

function restoreSelectedOngoingGroup(): OngoingGroup {
  const saved = localStorage.getItem(SELECTED_ONGOING_GROUP_KEY);
  return ONGOING_GROUPS.find(g => g === saved) ?? 'blocked';
}

const selectedOngoingGroupState = signal<OngoingGroup>(restoreSelectedOngoingGroup());

export const selectedOngoingGroup: ReadonlySignal<OngoingGroup> = selectedOngoingGroupState;

export function setSelectedOngoingGroup(group: OngoingGroup): void {
  selectedOngoingGroupState.value = group;
  ongoingFilterChoiceState.value = 'all';
  localStorage.setItem(SELECTED_ONGOING_GROUP_KEY, group);
}

export const ONGOING_PARTS = {
  review: ['changes', 'unread'],
  'in-flight': ['running', 'waiting'],
} as const satisfies Partial<Record<OngoingGroup, readonly string[]>>;
export type SplitOngoingGroup = keyof typeof ONGOING_PARTS;
export type OngoingPart = typeof ONGOING_PARTS[SplitOngoingGroup][number];

export type OngoingFilter = 'all' | OngoingPart;

export function isSplitOngoingGroup(group: OngoingGroup): group is SplitOngoingGroup {
  return group in ONGOING_PARTS;
}

const ongoingFilterChoiceState = signal<OngoingFilter>('all');

export const ongoingFilterChoice: ReadonlySignal<OngoingFilter> = ongoingFilterChoiceState;

export function setOngoingFilterChoice(filter: OngoingFilter): void {
  ongoingFilterChoiceState.value = filter;
}
export const DEFAULT_DRAWER_WIDTH = 300;
export { THREAD_DRAWER_WIDTH_KEY };

export const threadDrawerWidth = signal(
  Math.max(
    minDrawerWidth(),
    Number(localStorage.getItem(THREAD_DRAWER_WIDTH_KEY)) || DEFAULT_DRAWER_WIDTH,
  )
);

export function clampThreadDrawerWidth(): void {
  const min = minDrawerWidth();
  if (threadDrawerWidth.value >= min) return;
  threadDrawerWidth.value = min;
  localStorage.setItem(THREAD_DRAWER_WIDTH_KEY, String(min));
}
export const focusedThreadId = signal<string | null>(
  readGuardedFocus(FOCUSED_THREAD_KEY)
);

export function setFocusedThread(id: string | null): void {
  if (focusedThreadId.peek() === id) return;
  if (awaitedThreadId.peek() !== id) awaitedThreadId.value = null;
  focusedThreadId.value = id;
  armRestoreGuard(id);
  if (id) {
    writeWindowItem(FOCUSED_THREAD_KEY, id);
  } else {
    removeWindowItem(FOCUSED_THREAD_KEY);
  }
}

export const promptAnimating = signal(false);

export const promptSendCollapsing = signal(false);

export const revealOnFocus = signal(false);

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
    console.warn('[store] dropping malformed thread channel filter payload', err);
  }
  return new Set(ALL_CHANNELS);
}

export const threadChannelFilter = signal<Set<ThreadChannel>>(restoreThreadChannelFilter());

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

export function restoreIdSet(key: string): Set<string> {
  try {
    const saved = localStorage.getItem(key);
    if (!saved) return new Set();
    const parsed = JSON.parse(saved);
    if (!Array.isArray(parsed)) return new Set();
    return new Set(parsed.filter((x): x is string => typeof x === 'string'));
  } catch (err) {
    console.warn(`[store] dropping malformed ${key} payload`, err);
    return new Set();
  }
}

export function persistIdSet(key: string, next: ReadonlySet<string>): void {
  localStorage.setItem(key, JSON.stringify([...next]));
}

const SELECTED_TRIGGER_IDS_KEY = 'lucidos-selected-trigger-ids';

export const selectedTriggerIds = signal<Set<string>>(restoreIdSet(SELECTED_TRIGGER_IDS_KEY));

export function setSelectedTriggerIds(next: Set<string>): void {
  selectedTriggerIds.value = next;
  persistIdSet(SELECTED_TRIGGER_IDS_KEY, next);
}

const SELECTED_REPO_IDS_KEY = 'lucidos-selected-repo-ids';

export const selectedRepoIds = signal<Set<string>>(restoreIdSet(SELECTED_REPO_IDS_KEY));

export function setSelectedRepoIds(next: Set<string>): void {
  selectedRepoIds.value = next;
  persistIdSet(SELECTED_REPO_IDS_KEY, next);
}

const SELECTED_APP_IDS_KEY = 'lucidos-selected-app-ids';

export const selectedAppIds = signal<Set<string>>(restoreIdSet(SELECTED_APP_IDS_KEY));

export function setSelectedAppIds(next: Set<string>): void {
  selectedAppIds.value = next;
  persistIdSet(SELECTED_APP_IDS_KEY, next);
}

export function clearThreadFilter(): void {
  batch(() => {
    setThreadChannelFilter(new Set(ALL_CHANNELS));
    setSelectedTriggerIds(new Set());
    setSelectedRepoIds(new Set());
    setSelectedAppIds(new Set());
  });
}

const INCLUDE_DELETED_FILTERS_KEY = 'lucidos-filter-include-deleted';

export const includeDeletedFilterOptions = signal<boolean>(
  localStorage.getItem(INCLUDE_DELETED_FILTERS_KEY) === 'true'
);

export function setIncludeDeletedFilterOptions(next: boolean): void {
  includeDeletedFilterOptions.value = next;
  localStorage.setItem(INCLUDE_DELETED_FILTERS_KEY, String(next));
}

export const filterFacets = signal<Loadable<import('../api/threads').FilterFacets>>({ status: 'not-loaded' });

export const threadSearchQuery = signal('');
export const threadSearchResults = signal<Loadable<import('../api/threads').ThreadSearchResult[]>>({ status: 'not-loaded' });

export const threadMap = signal<Map<string, ThreadState>>(new Map());
export const threadsLoaded = signal(false);
export const threadListFetched = signal(false);
export const awaitedThreadId = signal<string | null>(null);
export const generatedTitleIds = new Set<string>();
export const threadHasMore = signal(true);
export const threadLoadingMore = signal(false);
export const archiveThreadCount = signal(0);
export function effectiveThreadStatus(thread: ThreadState): ThreadStatus {
  if (archivePending(thread.meta.id)) return 'idle';
  if (thread.pendingUserMessages.some(p => !p.unconfirmed)) return 'running';
  return thread.meta.status;
}

export function isMidTurn(status: ThreadStatus): boolean {
  return status === 'running' || status === 'waiting_for_user_answer';
}

export function threadUnsettled(thread: ThreadState): boolean {
  return isMidTurn(effectiveThreadStatus(thread)) || thread.meta.liveEventWaitCount > 0;
}

export function isThreadQuiescent(status: ThreadStatus | undefined): boolean {
  return status === 'idle' || status === 'waiting_for_user_answer';
}

export function isRenderedThreadIdle(thread: ThreadState | undefined): boolean {
  if (!thread) return false;
  if (answeringThreadIds.value.has(thread.meta.id)) return false;
  if (thread.pendingUserMessages.length > 0) return false;
  return isThreadQuiescent(thread.meta.status);
}

export function isThreadStreaming(thread: ThreadState): boolean {
  if (answeringThreadIds.value.has(thread.meta.id)) return true;
  return effectiveThreadStatus(thread) === 'running';
}

export function getThreadDisplaySection(thread: ThreadState): DisplaySection {
  return displaySection(
    thread.meta.section as ArchiveState,
    effectiveThreadStatus(thread),
    thread.meta.saved,
    thread.meta.activeChildrenCount > 0,
    changeReadyToReview(thread.meta),
    thread.meta.attentionDescendantCount > 0,
    threadIsHeld(thread),
  );
}

export function listsInOngoing(thread: ThreadState): boolean {
  if (isExcludedFromSections(thread) || threadIsHeld(thread)) return false;
  const section = getThreadDisplaySection(thread);
  return section === 'current' || section === 'saved';
}

export function threadIsHeld(thread: ThreadState): boolean {
  return thread.meta.hold != null;
}

export function threadAwaitsOwnerAnswer(thread: ThreadState): boolean {
  return !isExcludedFromSections(thread)
    && awaitsOwnerAnswer(effectiveThreadStatus(thread), threadIsHeld(thread));
}

export function threadIsBlocked(thread: ThreadState): boolean {
  if (threadAwaitsOwnerAnswer(thread)) return true;
  if (!listsInOngoing(thread)) return false;
  return effectiveThreadStatus(thread) === 'failed'
    || thread.meta.isStoppedChild === true
    || (!thread.meta.codingAgentIntegrated && notReadyReason(thread) === 'hardening_missing');
}

export function notReadyReason(thread: ThreadState | undefined): UnproposedReason | null {
  if (!thread || thread.meta.channel !== 'claude_code' || threadUnsettled(thread)) return null;
  const change = thread.meta.codingAgentChangeState;
  if (change.kind !== 'unproposed' || change.reason === null) return null;
  return agentHoldsUnreadMessage(thread) ? null : change.reason;
}

export type SuccessorOffer = 'warm' | 'cold';

export function successorOffer(
  thread: ThreadState | undefined,
  context: ThreadContext | null,
  now: number,
): SuccessorOffer | null {
  if (!thread || !context?.card || thread.meta.channel !== 'claude_code') return null;
  if (thread.meta.successor || thread.meta.section === 'archived') return null;
  if (effectiveThreadStatus(thread) !== 'idle' || thread.meta.liveEventWaitCount > 0) return null;
  if (thread.meta.codingAgentChangeState.kind !== 'none' || agentHoldsUnreadMessage(thread)) return null;
  const { warm, cold, cold_at: coldAt } = context.card;
  const state: SuccessorOffer = coldAt !== undefined && Date.parse(coldAt) > now ? 'warm' : 'cold';
  return (state === 'warm' ? warm : cold) ? state : null;
}

export const successorCardThreadId = signal<string | null>(null);

export const successorCardPick = signal<{ threadId: string; model: string; effort: string | null } | null>(null);

function agentHoldsUnreadMessage(thread: ThreadState): boolean {
  if (thread.meta.status !== 'idle') return false;
  const seqById = new Map<string, number>();
  const messageIds: string[] = [];
  const readIds: string[] = [];
  const removed = new Set<string>();
  for (const [seq, event] of thread.events) {
    if (event._eventId) seqById.set(event._eventId, seq);
    if (event.type === 'CodingAgentInputRead') readIds.push(event.input_event_id);
    else if (event.type === 'QueuedMessageRemoved') removed.add(event.removed_message_id);
    else if (event.type === 'MessageReceived' && event._eventId) messageIds.push(event._eventId);
  }
  const newestSeq = (ids: string[]) => ids.reduce((max, id) => Math.max(max, seqById.get(id) ?? -1), -1);
  const newestRead = newestSeq(readIds);
  return newestRead !== -1 && newestSeq(messageIds.filter(id => !removed.has(id))) > newestRead;
}

export const UNREAD_HOLD_EVENT_TYPES: ReadonlySet<string> = new Set(['CodingAgentInputRead', 'QueuedMessageRemoved']);

export function changeAwaitsApply(meta: Pick<ThreadMeta, 'id' | 'codingAgentChangeState'>): boolean {
  return changeReadyToReview(meta)
    && lanePlace(landingLane.value, pendingChangeForThread(meta.id)?.id) === null;
}

export function threadInReview(thread: ThreadState): boolean {
  if (!listsInOngoing(thread)) return false;
  if (changeAwaitsApply(thread.meta)) return !threadUnsettled(thread);
  return readRequestStanding(thread);
}

export function readRequestStanding(thread: ThreadState): boolean {
  return readRequestedForMe(thread.meta) && !isMidTurn(effectiveThreadStatus(thread));
}

export function readRequestedForMe(meta: Pick<ThreadMeta, 'readRequested' | 'replySeenDevices'>): boolean {
  if (meta.readRequested !== true) return false;
  const reader = readerDevices.value;
  const mine = reader.status === 'loaded' ? reader.data.devices : null;
  if (mine === null) return true;
  return !(meta.replySeenDevices ?? []).some((device) => mine.includes(device));
}

export const blockedThreadIds = computed<ReadonlySet<string>>(() => {
  const ids = new Set<string>();
  for (const thread of threadMap.value.values()) {
    if (threadIsBlocked(thread)) ids.add(thread.meta.id);
  }
  return ids;
});

export const blockedThreadCount = computed(() => blockedThreadIds.value.size);

export const activeExchanges = computed<Exchange[]>(() => {
  const id = focusedThreadId.value;
  if (!id) return [];
  getThreadEventsBump(id);
  const thread = threadMap.peek().get(id);
  if (!thread) return [];
  return computeExchanges(thread);
});

export const activeStreamingBuffer = computed(() => {
  const id = focusedThreadId.value;
  if (!id) return '';
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

export const composeViewActive = computed(() => {
  const id = focusedThreadId.value;
  const isEmpty = activeExchanges.value.length === 0;
  return (!id && isEmpty) || activeThreadIsComposing.value || homeWelcomeActive.value;
});

export const welcomeDismissed = computed(() => {
  const prefs = preferences.value;
  return prefs.status !== 'loaded' || prefs.data['welcome_suggestions_dismissed'] === 'true';
});

export const homeWelcomeActive = computed(() => {
  const id = focusedThreadId.value;
  if (!id) return false;
  const thread = threadMap.value.get(id);
  return !!thread?.meta.home
    && thread.eventsLoaded
    && activeExchanges.value.length === 0
    && !welcomeDismissed.value;
});

export const SPLIT_RATIO_KEY = 'lucidos-split-ratio';
const storedSplitRatio = parseFloat(localStorage.getItem(SPLIT_RATIO_KEY) ?? '');
export const splitRatio = signal(
  Number.isFinite(storedSplitRatio) ? storedSplitRatio : 0.4,
);

export type FocusedPane = 'drawer' | 'thread' | 'content';
export const focusedPane = signal<FocusedPane>('thread');
const PANE_DEFS = ['threads', 'thread', 'content'] as const;
export type MobileView = typeof PANE_DEFS[number];
export const MOBILE_VIEWS: MobileView[] = [...PANE_DEFS];
export const PANE_INDEX: Record<MobileView, number> =
  Object.fromEntries(PANE_DEFS.map((v, i) => [v, i])) as Record<MobileView, number>;
export const PANE_COUNT = PANE_DEFS.length;
export const MOBILE_VIEW_KEY = 'lucidos-mobile-view';

export function getInitialMobileView(): MobileView {
  const saved = readWindowItem(MOBILE_VIEW_KEY);
  return saved && (MOBILE_VIEWS as string[]).includes(saved) ? (saved as MobileView) : 'thread';
}

export const mobileView = signal<MobileView>(getInitialMobileView());

export function setMobileView(view: MobileView) {
  mobileView.value = view;
  writeWindowItem(MOBILE_VIEW_KEY, view);
}

export type InputMode =
  | { type: 'do' }
  | { type: 'coding_agent' };

function restoreInputMode(): InputMode {
  try {
    const raw = localStorage.getItem('lucidos-input-mode');
    if (!raw) return { type: 'do' };
    const parsed = JSON.parse(raw) as { type?: unknown };
    if (parsed?.type === 'coding_agent' || parsed?.type === 'claude_code') {
      return { type: 'coding_agent' };
    }
    return { type: 'do' };
  } catch (err) {
    console.warn('[store] dropping malformed lucidos-input-mode payload', err);
    return { type: 'do' };
  }
}
export const inputMode = signal<InputMode>(restoreInputMode());

export interface Repository {
  id: string;
  name: string;
  path: string;
  description?: string;
}

export const repositories = signal<Loadable<Repository[]>>({ status: 'not-loaded' });

export type Scope =
  | { kind: 'lucidos' }
  | { kind: 'external'; repoId: string }
  | { kind: 'app'; appId: string };

const SCOPE_STORAGE_KEY = 'lucidos-coding-agent-last-scope';
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
  }
  return null;
}

function restoreScope(): Scope {
  const current = parseStoredScope(localStorage.getItem(SCOPE_STORAGE_KEY));
  if (current) return current;

  if (localStorage.getItem(LEGACY_SCOPE_STORAGE_KEY) !== null) {
    const renamed = parseStoredScope(localStorage.getItem(LEGACY_SCOPE_STORAGE_KEY));
    localStorage.removeItem(LEGACY_SCOPE_STORAGE_KEY);
    if (renamed) {
      localStorage.setItem(SCOPE_STORAGE_KEY, JSON.stringify(renamed));
      return renamed;
    }
  }

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

export const selectedCodingAgent = signal<import('../api/types').CodingAgent>(
  PREF_CODING_AGENT_DEFAULT.fallback,
);

export function scopeToFolder(scope: Scope): string {
  switch (scope.kind) {
    case 'lucidos': return '';
    case 'external': return scope.repoId;
    case 'app': return `data/apps/${scope.appId}`;
  }
}

export function scopeToRepoId(scope: Scope): string | undefined {
  return scope.kind === 'external' ? scope.repoId : undefined;
}

export const repoSource = signal<string | null>(null);
export const repoFiles = signal<Loadable<string[]>>({ status: 'not-loaded' });
export const repoExpandedFolders = signal<Set<string>>(new Set());
export const revealedFolder = signal<string | null>(null);

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

export interface LineScrollTarget {
  path: string;
  line: number;
}

export const lineScrollTarget = signal<LineScrollTarget | null>(null);

export interface FindRequest {
  scope: string;
  query: string;
  line: number;
}

export const findRequest = signal<FindRequest | null>(null);

export function consumeLineScrollTarget(
  pending: Signal<LineScrollTarget | null>, path: string | null,
): number | null {
  const target = pending.value;
  if (target === null || target.path !== path) return null;
  pending.value = null;
  return target.line;
}

export interface PreviewViewState {
  selectedLines: Signal<{ start: number; end: number } | null>;
  lineScrollTarget: Signal<LineScrollTarget | null>;
  source: Signal<boolean>;
  editing: Signal<boolean>;
}

export const panelPreviewViewState: PreviewViewState = {
  selectedLines,
  lineScrollTarget,
  source: filePreviewSource,
  editing: filePreviewEditing,
};

export function createPreviewViewState(): PreviewViewState {
  return {
    selectedLines: signal(null),
    lineScrollTarget: signal(null),
    source: signal(false),
    editing: signal(false),
  };
}

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
export const repoSelectedChangeId = signal<string | null>(readWindowItem(SELECTED_CHANGE_KEY));
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

export { encodeRepoPath, parseRepoPath, type RepoLocator } from './repoPath';

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

export const applyingChangeIds = signal<Set<string>>(new Set());
export const applyNowRequests = signal<ReadonlyMap<string, number | null>>(new Map());
export const applyingNowThreadIds = computed<ReadonlySet<string>>(() => {
  const applying = new Set<string>();
  const threads = threadMap.value;
  for (const [id, thread] of threads) {
    if (thread.meta.applyInFlight) applying.add(id);
  }
  for (const [id, asked] of applyNowRequests.value) {
    const version = threads.get(id)?.meta.summaryVersion;
    if (asked === null || version === undefined || version < asked) applying.add(id);
  }
  return applying;
});
export const applyAllInProgress = signal(false);
export interface ApplyAllBatch {
  changeIds: readonly string[];
  resolvedChangeIds: readonly string[];
  applyingChangeIds: readonly string[];
  resolvingChangeIds: readonly string[];
}
export const applyAllBatch = signal<ApplyAllBatch | null>(null);
export const applyAllCanceling = signal(false);
export const EMPTY_LANDING_LANE: LandingLane = { open_batch: null, waiting: [], gathering: false };
export const landingLane = signal<LandingLane>(EMPTY_LANDING_LANE);
export type LanePlace = 'in-batch' | 'waiting';
export function lanePlace(lane: LandingLane, changeId: string | undefined): LanePlace | null {
  if (!changeId) return null;
  if (lane.open_batch?.members.includes(changeId)) return 'in-batch';
  if (lane.waiting.includes(changeId)) return 'waiting';
  return null;
}
export const landingLaneCanceling = signal<string | null>(null);
export const IDLE_MAIN_CHECK: MainCheck = { state: 'idle' };
export const mainCheck = signal<MainCheck>(IDLE_MAIN_CHECK);
export const mainCheckClosing = signal<string | null>(null);
export const APPLY_ALL_SUMMARY_TOAST_KEY = 'apply-all-summary';
export type ApplyPhase = 'merging' | 'resolving-conflict' | 'hardening';
export interface ApplyPhaseReading {
  phase: ApplyPhase;
  eventId: string | null;
  startedAt: string | null;
}
export const applyPhases = signal<ReadonlyMap<string, ApplyPhaseReading>>(new Map());
export const NO_APPLY_ESTIMATES: ApplyEstimates = { hardening: null, resolving_conflict: null };
export const applyEstimates = signal<ApplyEstimates>(NO_APPLY_ESTIMATES);
export const standingApplyThreadIds = signal<Set<string>>(new Set());
export const armingStandingApplyThreadIds = signal<Set<string>>(new Set());
export const discardingCCThreadIds = signal<Set<string>>(new Set());
export const cancelingThreadIds = signal<Set<string>>(new Set());
export const removingQueuedMessageIds = signal<Set<string>>(new Set());
export const queuedMessageRemovalKey = (threadId: string, messageId: string): string => `${threadId}:${messageId}`;
export const answeringThreadIds = signal<Set<string>>(new Set());

export function markThreadAnswering(threadId: string): void {
  if (answeringThreadIds.value.has(threadId)) return;
  const next = new Set(answeringThreadIds.value);
  next.add(threadId);
  answeringThreadIds.value = next;
}

export function clearThreadAnswering(threadId: string): void {
  if (!answeringThreadIds.value.has(threadId)) return;
  const next = new Set(answeringThreadIds.value);
  next.delete(threadId);
  answeringThreadIds.value = next;
}
export const changes = signal<Loadable<Change[]>>({ status: 'not-loaded' });
export const appliedChanges = signal<Loadable<Change[]>>({ status: 'not-loaded' });
export const setAsideChanges = signal<Loadable<Change[]>>({ status: 'not-loaded' });
export function pendingChangeForThread(threadId: string): Change | undefined {
  const loadable = changes.value;
  return loadable.status === 'loaded'
    ? loadable.data.find((c) => c.thread_id === threadId && c.status === 'pending')
    : undefined;
}
export function setAsideChangeForThread(threadId: string): Change | undefined {
  const loadable = setAsideChanges.value;
  return loadable.status === 'loaded'
    ? loadable.data.find((c) => c.thread_id === threadId)
    : undefined;
}
export const lazyChanges = signal<Map<string, Loadable<Change>>>(new Map());
export function findChangeById(id: string): Change | undefined {
  if (changes.value.status === 'loaded') {
    const pending = changes.value.data.find(c => c.id === id);
    if (pending) return pending;
  }
  if (appliedChanges.value.status === 'loaded') {
    const applied = appliedChanges.value.data.find(c => c.id === id);
    if (applied) return applied;
  }
  if (setAsideChanges.value.status === 'loaded') {
    const setAside = setAsideChanges.value.data.find(c => c.id === id);
    if (setAside) return setAside;
  }
  const lazy = lazyChanges.value.get(id);
  return lazy?.status === 'loaded' ? lazy.data : undefined;
}
export function changeIsReady(change: Change): boolean {
  return change.apply_ready;
}

export const actionableChangeCount = computed<number | null>(() => {
  const loadable = changes.value;
  if (loadable.status !== 'loaded') return null;
  const inFlight = inFlightChangeIds.value;
  return loadable.data.filter((c) => changeIsReady(c) && !inFlight.has(c.id)).length;
});
export const busyChangeIds = computed(() => {
  const ids = new Set(applyingChangeIds.value);
  const batch = applyAllBatch.value;
  if (batch) {
    const resolved = new Set(batch.resolvedChangeIds);
    for (const id of batch.changeIds) if (!resolved.has(id)) ids.add(id);
  }
  const threadIds = applyingNowThreadIds.value;
  if (threadIds.size > 0 && changes.value.status === 'loaded') {
    for (const c of changes.value.data) {
      if (c.thread_id && threadIds.has(c.thread_id)) ids.add(c.id);
    }
  }
  return ids;
});
export const inFlightChangeIds = computed(() => {
  const ids = new Set(busyChangeIds.value);
  const lane = landingLane.value;
  for (const id of lane.waiting) ids.add(id);
  for (const id of lane.open_batch?.members ?? []) ids.add(id);
  return ids;
});
export const applyingChangeThreadIds = computed(() => {
  const result = new Set<string>();
  const ids = applyingChangeIds.value;
  if (ids.size === 0 || changes.value.status !== 'loaded') return result;
  for (const c of changes.value.data) {
    if (c.thread_id && ids.has(c.id)) result.add(c.thread_id);
  }
  return result;
});
export const changesHasMore = signal(false);
export const changesLoadingMore = signal(false);
export const restartRequired = signal(false);
export const engineRestarting = signal(false);
export interface RestartGroup {
  threadId: string;
  threadTitle: string;
  commits: string[];
}

export const restartGroups = signal<RestartGroup[]>([]);
export const STEPS_EXPANDED_KEY = 'lucidos-steps-expanded-v2';
export const DETAILS_EXPANDED_KEY = 'lucidos-details-expanded-v2';

export function seedTurnControl(stored: string | null): boolean {
  return stored !== 'false';
}

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

export const artifacts = signal<Loadable<string[]>>({ status: 'not-loaded' });
export const filePreviewRevision = signal<{ path: string; rev: number } | null>(null);
export const openFilePreviewRevision = computed(() => {
  const stamp = filePreviewRevision.value;
  return stamp && stamp.path === previewFile.value ? stamp.rev : 0;
});
export const panelTitle = signal<string | null>(null);
export const webviewInitialUrl = signal<string | null>(null);
export const fileSearchOpen = signal(false);
export const fileSearchAnchor = signal<HTMLElement | null>(null);

export const searchEverywhereOpen = signal(false);

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

export const servedNotifications = signal<Loadable<Notification[]>>({ status: 'not-loaded' });
export const servedUnreadNotifications = signal<Loadable<Notification[]>>({ status: 'not-loaded' });
export const pendingNotificationReads = signal<PendingReads>(new Map());
export const servedNotificationEpochs = signal<{ browse: number; unread: number }>({ browse: 0, unread: 0 });
export const notifications = computed(() => withPendingReadsShown(
  servedNotifications.value, pendingNotificationReads.value, servedNotificationEpochs.value.browse,
));
export const unreadNotifications = computed(() => withoutPendingReads(
  servedUnreadNotifications.value, pendingNotificationReads.value, servedNotificationEpochs.value.unread,
));
export const unreadCount = computed(() =>
  unreadNotifications.value.status === 'loaded' ? unreadNotifications.value.data.length : 0,
);
export type NotificationsFilter = PreferenceValues<'notifications_filter'>;
export const NOTIFICATIONS_FILTER_STORAGE_KEY = 'lucidos-notifications-filter';
const cachedFilter = localStorage.getItem(NOTIFICATIONS_FILTER_STORAGE_KEY);
export const notificationsFilter = signal<NotificationsFilter>(
  PREF_NOTIFICATIONS_FILTER.values.find((f) => f === cachedFilter)
    ?? PREF_NOTIFICATIONS_FILTER.fallback,
);
export const notificationsHasMore = signal(false);
export const notificationsLoadingMore = signal(false);
export const pageTitle = computed(() =>
  documentTitle(visibleWorkspaceName.value, unreadCount.value)
);
export const credentials = signal<Loadable<CredentialInfo[]>>({ status: 'not-loaded' });

export const environmentVariables = signal<Loadable<EnvironmentVariable[]>>({ status: 'not-loaded' });

export const chatModels = signal<Loadable<ModelInfo[]>>({ status: 'not-loaded' });

export const chatLadder = signal<Loadable<IntelligenceLadder>>({ status: 'not-loaded' });

export const responseStyles = signal<Loadable<ResponseStyle[]>>({ status: 'not-loaded' });

export const oauthAccounts = signal<Loadable<OAuthAccountInfo[]>>({ status: 'not-loaded' });

export const knownOAuthProviders = signal<Loadable<KnownOAuthProviders>>({ status: 'not-loaded' });

export const oauthConnectPrefill = signal<{ provider: string; scopes?: string } | null>(null);

export const triggers = signal<Loadable<TriggerInfo[]>>({ status: 'not-loaded' });

export const threadQueue = signal<Loadable<ThreadQueueResponse>>({ status: 'not-loaded' });

export const historicalTriggers = signal<Loadable<HistoricalTriggerInfo[]>>({ status: 'not-loaded' });

export const triggerGroups = signal<Loadable<TriggerGroup[]>>({ status: 'not-loaded' });

export const UNGROUPED_TRIGGER_SECTION_ID = '__ungrouped__';

const COLLAPSED_TRIGGER_SECTIONS_KEY = 'lucidos-collapsed-trigger-groups';

export const collapsedTriggerSectionIds = signal<Set<string>>(restoreIdSet(COLLAPSED_TRIGGER_SECTIONS_KEY));

function persistCollapsedTriggerSections(next: Set<string>): void {
  collapsedTriggerSectionIds.value = next;
  persistIdSet(COLLAPSED_TRIGGER_SECTIONS_KEY, next);
}

export function toggleTriggerSectionCollapsed(sectionId: string): void {
  const next = new Set(collapsedTriggerSectionIds.value);
  if (next.has(sectionId)) next.delete(sectionId);
  else next.add(sectionId);
  persistCollapsedTriggerSections(next);
}

export function expandTriggerSection(sectionId: string): void {
  if (!collapsedTriggerSectionIds.value.has(sectionId)) return;
  const next = new Set(collapsedTriggerSectionIds.value);
  next.delete(sectionId);
  persistCollapsedTriggerSections(next);
}

function collapsedSectionsStore<Id extends string>(key: string) {
  const ids = signal<Set<string>>(restoreIdSet(key));
  function toggle(id: Id): void {
    const next = new Set(ids.value);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    ids.value = next;
    persistIdSet(key, next);
  }
  return { ids, toggle };
}

export type ChangesSectionId = 'applying' | 'ready' | 'not-finished' | 'set-aside' | 'applied';
export const {
  ids: collapsedChangesSectionIds,
  toggle: toggleChangesSectionCollapsed,
} = collapsedSectionsStore<ChangesSectionId>('lucidos-collapsed-changes-sections');

export type ThreadQueueSectionId = 'running' | 'queued';
export const {
  ids: collapsedThreadQueueSectionIds,
  toggle: toggleThreadQueueSectionCollapsed,
} = collapsedSectionsStore<ThreadQueueSectionId>('lucidos-collapsed-thread-queue-sections');

export const triggerScrollTarget = signal<string | null>(null);

export const pendingChatMessage = signal<string | null>(null);

export const codingAgentSessionVersion = signal(0);

export const appsList = signal<Loadable<App[]>>({ status: 'not-loaded' });
export const marketplaceCatalog = signal<Loadable<MarketplaceCatalog>>({ status: 'not-loaded' });
export const marketplaceScanning = signal(false);
export const installedPlugins = signal<Loadable<InstalledPlugin[]>>({ status: 'not-loaded' });
export const appRefreshKey = signal(0);
export const wipPreviewThreadId = signal<string | null>(null);
export const pinnedApps = signal<Loadable<PinnedAppEntry[]>>(
  hydratePinnedAppsFromStorage(),
);

const PLUGINS_INSTALLED_ONLY_KEY = 'lucidos-plugins-installed-only';
export const pluginsInstalledOnly = signal<boolean>(
  localStorage.getItem(PLUGINS_INSTALLED_ONLY_KEY) === 'true',
);
export function setPluginsInstalledOnly(next: boolean): void {
  pluginsInstalledOnly.value = next;
  localStorage.setItem(PLUGINS_INSTALLED_ONLY_KEY, String(next));
}

export const pluginsMarketplaceFilter = signal<string | null>(null);
export const pluginScrollTarget = signal<string | null>(null);
export const appSearchOpen = signal(false);
export const appSearchQuery = signal('');


export const preferences = signal<Loadable<Record<string, string>>>({ status: 'not-loaded' });

const CONFIRM_CLOSED: ConfirmState = { visible: false, message: '', okLabel: 'Delete' };
export const confirmState = signal<ConfirmState>(CONFIRM_CLOSED);

export function settleConfirm(value: boolean): void {
  confirmState.peek().resolve?.(value);
  confirmState.value = CONFIRM_CLOSED;
}

export const progressDialog = signal<ProgressDialogState>({
  visible: false,
  title: '',
  message: '',
  progress: null,
});

export const engineRestartNewVersion = signal(false);

export const connectionNoticeStatus = computed<ConnectionNoticeStatus>(() => {
  if (!engineRestarting.value) return connectionStatus.value;
  return engineRestartNewVersion.value ? 'switching-version' : 'restarting';
});

async function requestAppUpdateCancel(): Promise<void> {
  try {
    await cancelAppUpdate();
  } catch (e) {
    showToast(`Could not cancel the update: ${errorDetail(e)}`, 'error');
  }
}

export const activeProgressDialog = computed<ProgressDialogState>(() => {
  if (engineRestarting.value && restartTakesPageDown({ servedByGateway: WORKSPACE_ID !== null, restartsGateway: restartRestartsGateway() })) {
    return restartDialogState(engineRestartNewVersion.value);
  }
  const frame = appUpdateProgress.value;
  if (frame) {
    return appUpdateDialogState(
      frame,
      relayedUpdate.value ? null : () => { void requestAppUpdateCancel(); },
    );
  }
  return progressDialog.value;
});

const PROMPT_CLOSED: PromptState = { visible: false, message: '' };
export const promptState = signal<PromptState>(PROMPT_CLOSED);

export function settlePrompt(value: string | null): void {
  promptState.peek().resolve?.(value);
  promptState.value = PROMPT_CLOSED;
}

export interface FilePreviewModalState {
  id: number;
  path: string;
  range: { start: number; end: number } | null;
  view: PreviewViewState;
}
export const filePreviewModal = signal<FilePreviewModalState | null>(null);

let toastIdCounter = 0;
export const toasts = signal<ToastItem[]>([]);
export const TOAST_AUTO_DISMISS_MS = 5_000;

export function defaultToastDurationMs(type: ToastType, waits: boolean): number | undefined {
  return waits || type === 'error' || type === 'warning' ? undefined : TOAST_AUTO_DISMISS_MS;
}
const keyedDismissTimers = new Map<string, ReturnType<typeof setTimeout>>();

export function workspaceUnavailable(): boolean {
  return engineRestarting.value || appUpdateCommitted.value || !databaseReachable.value;
}

export function showToast(rawMessage: string, type: ToastType = 'info', opts?: { title?: string; key?: string; action?: ToastAction; secondaryAction?: ToastAction; onClick?: () => void; spinning?: boolean; progress?: number | null; autoDismissMs?: number; dismissable?: boolean; showWhileUnavailable?: boolean }) {
  const { key, action, secondaryAction, onClick, spinning, progress, autoDismissMs, dismissable, showWhileUnavailable } = opts ?? {};
  const message = clampToastText(rawMessage, type);
  const title = opts?.title?.trim() ? clampToastText(opts.title, type) : undefined;
  if (workspaceUnavailable() && !showWhileUnavailable) return;
  const autoMs = key
    ? autoDismissMs
    : autoDismissMs ?? defaultToastDurationMs(type, Boolean(action || onClick));
  const persistent = autoMs === undefined;
  if (key) {
    const existing = toasts.value.find((t) => t.key === key);
    if (existing) {
      toasts.value = toasts.value.map((t) => t.key === key ? { ...t, title, message, type, action, secondaryAction, onClick, spinning, progress, dismissable, persistent } : t);
      scheduleAutoDismiss(key, autoMs);
      return;
    }
  }
  const plain = !key && !action && !secondaryAction && !onClick && !spinning && progress == null;
  const twin = plain
    ? toasts.value.find((t) => !t.key && !t.action && !t.secondaryAction && !t.onClick && !t.spinning
        && t.progress == null && t.type === type && t.title === title && t.message === message)
    : undefined;
  if (twin) {
    toasts.value = toasts.value.map((t) => t === twin ? { ...t, count: (t.count ?? 1) + 1, persistent } : t);
    scheduleUnkeyedDismiss(twin.id, autoMs);
    return;
  }
  const id = ++toastIdCounter;
  toasts.value = [{ id, title, message, type, key, action, secondaryAction, onClick, spinning, progress, dismissable, persistent }, ...toasts.value];
  if (key) {
    scheduleAutoDismiss(key, autoMs);
    return;
  }
  scheduleUnkeyedDismiss(id, autoMs);
}

const unkeyedDismissTimers = new Map<number, ReturnType<typeof setTimeout>>();

function scheduleUnkeyedDismiss(id: number, autoDismissMs: number | undefined): void {
  clearTimeout(unkeyedDismissTimers.get(id));
  unkeyedDismissTimers.delete(id);
  if (autoDismissMs === undefined) return;
  unkeyedDismissTimers.set(id, setTimeout(() => dismissToast(id), autoDismissMs));
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

export function removeToast(key: string): void {
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
    scheduleUnkeyedDismiss(idOrKey, undefined);
    return;
  }
  removeToast(idOrKey);
  if (idOrKey === 'update-available') {
    markSwUpdateDismissed();
  } else if (idOrKey === NEW_VERSION_TOAST_KEY) {
    markEngineVersionDismissed();
  } else if (idOrKey === BUILD_FAILED_TOAST_KEY) {
    markBuildFailureDismissed();
  }
}

export type ConfirmOptions = ConfirmAlternative & {
  variant: ConfirmVariant;
  title?: string;
  cancelLabel?: string;
  details?: ConfirmDetails;
  acknowledge?: boolean;
};

let askDialogChunk: ChunkGate | null = null;
export function registerAskDialogChunk(chunk: ChunkGate | null): void {
  askDialogChunk = chunk;
}

export async function ensureAskDialogsReady(): Promise<void> {
  if (askDialogChunk && !askDialogChunk.isLoaded() && !await askDialogChunk.preload()) {
    throw new Error('Could not load the dialog. Reopen Lucidos and retry.');
  }
}

function whenAskDialogsDraw(show: () => void, decline: () => void, current: () => boolean): void {
  const chunk = askDialogChunk;
  if (!chunk || chunk.isLoaded()) {
    show();
    return;
  }
  void chunk.preload().then((loaded) => {
    if (loaded && current()) show();
    else decline();
  });
}

let confirmCalls = 0;
let promptCalls = 0;

export function showConfirm(
  message: string,
  okLabel: string,
  options: ConfirmOptions,
): Promise<boolean> {
  const prior = confirmState.peek();
  if (prior.visible) prior.resolve?.(false);
  const call = ++confirmCalls;

  return new Promise((resolve) => {
    whenAskDialogsDraw(() => {
      confirmState.value = {
        visible: true,
        message,
        okLabel,
        title: options.title,
        cancelLabel: options.cancelLabel,
        variant: options.variant,
        resolve,
        ...(options.choices ? { choices: options.choices } : { extraAction: options.extraAction }),
        details: options.details,
        acknowledge: options.acknowledge,
      };
    }, () => resolve(false), () => call === confirmCalls);
  });
}

export function showPrompt(
  message: string,
  options?: {
    title?: string;
    defaultValue?: string;
    placeholder?: string;
    okLabel?: string;
    cancelLabel?: string;
    multiline?: boolean;
    suggest?: PromptState['suggest'];
  }
): Promise<string | null> {
  const prior = promptState.peek();
  if (prior.visible) prior.resolve?.(null);
  holdSoftwareKeyboard();
  const call = ++promptCalls;

  return new Promise((resolve) => {
    whenAskDialogsDraw(() => {
      promptState.value = {
        visible: true,
        message,
        title: options?.title,
        defaultValue: options?.defaultValue,
        placeholder: options?.placeholder,
        okLabel: options?.okLabel,
        cancelLabel: options?.cancelLabel,
        multiline: options?.multiline,
        suggest: options?.suggest,
        resolve,
      };
    }, () => resolve(null), () => call === promptCalls);
  });
}

export type StepDetailModalState = Extract<ResponseEvent, { type: 'step' }> | null;
export const stepDetailModal = signal<StepDetailModalState>(null);

export const OPEN_SUB_AGENT_GROUPS_KEY = 'lucidos-open-sub-agent-groups';
export const openSubAgentGroups = signal<ReadonlySet<string>>(loadStringSet(OPEN_SUB_AGENT_GROUPS_KEY));
export function toggleSubAgentGroup(toolUseId: string): void {
  const next = new Set(openSubAgentGroups.value);
  if (!next.delete(toolUseId)) next.add(toolUseId);
  openSubAgentGroups.value = next;
}

export type CheckpointDiffModalState = Extract<ResponseEvent, { type: 'checkpoint' }> | null;
export const checkpointDiffModal = signal<CheckpointDiffModalState>(null);

export interface ContextViewerState {
  snapshot: ContextCapture;
  description?: string;
}
export const contextViewer = signal<ContextViewerState | null>(null);

export interface EventCondition {
  eventType: string;
  conditions: Record<string, unknown>[];
}
export interface EventConditionPopoverState extends EventCondition {
  anchor: HTMLElement;
  waitId: string;
}
export const eventConditionPopover = signal<EventConditionPopoverState | null>(null);

export function eventConditionPopoverOpenFor(waitId: string, eventType: string): boolean {
  const open = eventConditionPopover.value;
  return open?.waitId === waitId && open.eventType === eventType;
}

export function toggleEventConditionPopover(state: EventConditionPopoverState): void {
  eventConditionPopover.value = eventConditionPopoverOpenFor(state.waitId, state.eventType)
    ? null
    : state;
}

export function eventConditionDoor(
  g: SubscriptionGroup,
): { label: string; condition: EventCondition } | null {
  const { event_type: eventType, conditions } = g;
  if (conditions.length === 0) return null;
  const condition = { eventType, conditions };
  const plainName = plainEventName(eventType);
  return {
    label: conditions.length === 1
      ? `${plainName} · show the condition`
      : `${plainName} · show the ${conditions.length} conditions`,
    condition,
  };
}

export * from './imagePopup';
export * from './messageRoutePanel';
export * from './backgroundActivity';

export const memoryRebuildProgress = gapClearedProgress<{ processed: number; total: number; percent: number }>();

export const backupProgress = gapClearedProgress<{ phase: string; progress: number; total: number }>();

export const backupStatusVersion = gapRefreshedVersion();

export const lastSuccessfulBackup = signal<Loadable<LastSuccessfulBackupResponse>>({ status: 'not-loaded' });
export const snapshotsVersion = gapRefreshedVersion();

export const backupRecipientVersion = gapRefreshedVersion();

export const recommendedCleanupProgress = gapClearedProgress<{ done: number; total: number }>();

export const diskUsageVersion = gapRefreshedVersion();

export const promptFootprintVersion = gapRefreshedVersion();

export const backgroundModelsVersion = gapRefreshedVersion();

export const backupPreferencesVersion = gapRefreshedVersion();

export const intelligenceLadderVersion = gapRefreshedVersion();

export const mcpServersVersion = gapRefreshedVersion();

export const responseStylesVersion = gapRefreshedVersion();

export const webhooksVersion = gapRefreshedVersion();

export const eventHooksVersion = gapRefreshedVersion();

export const memoryEntriesVersion = gapRefreshedVersion();

export const summaryTreesVersion = gapRefreshedVersion();

export const webhookIngress = signal<Loadable<IngressReading>>({ status: 'not-loaded' });

export const webhookRefusals = signal<Loadable<RefusalReading>>({ status: 'not-loaded' });

export const permissionGrantsVersion = gapRefreshedVersion();

export const handshakeScriptsVersion = gapRefreshedVersion();

export const recoveryProgress = gapClearedProgress<{ completed: number; total: number }>();

export const updateAvailable = signal(false);

export const engineVersionReady = signal(false);

export const engineVersionPending = signal(false);

export const engineBuildDeferred = signal<(DeferredBuild & { anchoredAt: number }) | null>(null);
export const engineUnbuiltHead = signal<string | null>(null);

export const engineBuilding = signal(false);

export const enginePendingCommits = signal<PendingCommits | null>(null);

export function engineNewVersionReady(): boolean {
  return engineVersionReady.value || (enginePackaged.value === true && restartRequired.value);
}

export const NEW_VERSION_TOAST_KEY = 'engine-new-version';

export const BUILD_FAILED_TOAST_KEY = 'engine-build-failed';

export const THREAD_LIST_REFRESH_TOAST_KEY = 'thread-list-refresh-failed';

export const THREAD_EVENTS_LOAD_TOAST_KEY = 'thread-events-load-failed';
export const THREAD_EVENTS_REFRESH_TOAST_KEY = 'thread-events-refresh-failed';

export const THREAD_EVENTS_FETCH_CONCURRENCY = 4;

export const THREAD_EVENTS_PREFETCH_LIMIT = 8;

export const CONNECTION_POLL_INTERVAL_MS = 5000;

export const HEALTH_PROBE_TIMEOUT_MS = 4500;

export const FRONTEND_UPDATE_DEFERRED_TOAST_KEY = 'engine-frontend-update-deferred';

export const FRONTEND_UPDATE_STRANDED_TOAST_KEY = 'engine-frontend-update-stranded';

export const serviceWorkerBuildId = signal<string | null>(null);
