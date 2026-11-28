import { PREF_EXTERNAL_LINK_TARGET, PREF_IMAGE_MODEL } from '@lucidos/preference-catalog';
import { useState, useEffect, useCallback, useRef } from 'preact/hooks';
import { aliasRetiredSettingsAnchor, currentModel, preferences, showToast, showConfirm, oauthAccounts, credentials, settingsSubview, settingsScrollTarget, settingsLinkedFragment, settingsSubviewLabel, repositories, knownOAuthProviders, oauthConnectPrefill, agentSandboxUnavailable, agentSandboxTruncationUnguarded, agentSandboxMetadataUnguarded } from '../../store/store';
import { devices, getDeviceId, updateDeviceName, removeDevice } from '../../store/actions/devices';
import { currentAgentIconColors, setAgentIconColors, type AgentIconColors, setImageModel, setThemeMode, setFontFamily, setChatModelSelection, setTriggerModelSelection, currentTriggerModel, accountChatEffort, currentThemeMode, currentFontFamily, currentUiScale, currentImageModel,  currentMemoryModule, currentCommandGuard, setCommandGuard, currentCommandGuardJudge, setCommandGuardJudge, currentCodingAgentAutoAnswer, setCodingAgentAutoAnswer, currentAgentsUnsandboxed, setAgentsUnsandboxed, currentNotificationToasts, setNotificationToasts, currentInAppBrowser, setInAppBrowser, currentExternalLinkTarget, setExternalLinkTarget, externalLinkTargetConfigurable, currentMaxToolCalls, setMaxToolCalls, estimateTurnDuration, MAX_TOOL_CALLS_DEFAULT, MAX_TOOL_CALLS_MIN, MAX_TOOL_CALLS_MAX, currentStyleOverrides, clearStyleOverrides, setMotion, setThemeEffects, type ExternalLinkTarget, type ThemeMode, type FontPreference } from '../../store/actions/preferences';
import { DEFAULT_THEME_MODE, THEME_EFFECTS_PREFS, THEME_MODES, MOTION_PREFS, type ThemeEffectsPref, type MotionPref } from '@lucidos/appearance';

import { openScaleModal } from '../shared/scaleModalState';
import { applyNavFocus } from '../shared/focusMarker';
import { formatDateTime, formatShortDateWithYear } from '../../utils/formatTime';
import {
  loadKnownOAuthProviders,
  disconnectOAuthAccount,
  grantOAuthScope,
} from '../../store/actions/oauth';
import {
  missingScopes,
  prefillLabel,
  providerToSend,
  reconnectScopes,
} from './oauthConnectForm';
import {
  ProviderPermissionsHint,
  reauthorizationHint,
} from '../credentials/providerConsoleHint';
import { handleNavigationRequest } from '../../store/actions/navigation-request';
import { settingsLinkAnchor } from '../search/searchIndex';
import { setDevicePushEnabled } from '../../store/actions/push';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { SETTINGS_SECTION_REFRESH } from './settingsSectionRefresh';
import {
  lucidosModelChoices, LUCIDOS_TIER_VOCABULARY, PROVIDER_DEFAULT_EFFORT_LABEL, rememberProvider,
  defaultTierLabel, modelDefaultEffort,
} from '../../store/actions/models';
import { type ModelChoice } from '../../store/modelSelection';
import { ModelSelectionRow } from './ModelSelectionRow';
import { BackgroundModelRow } from './BackgroundModelRow';
import { TitleStyleRow } from './TitleStyleRow';
import { useBackgroundModels } from './useBackgroundModels';
import { ResponseStylesSection } from './ResponseStylesSection';
import { ModelsManager } from './ModelsManager';
import { ProvidersPage } from './ProvidersPage';
import { VoiceSection } from './VoiceSection';
import { JudgmentModelRow } from './JudgmentModelRow';
import { Dropdown } from '../shared/Dropdown';
import { fontOptions } from './fontOptions';
import { WorkspaceFontsSection } from './WorkspaceFontsSection';
import { workspaceFontList } from '../../store/actions/workspaceFonts';
import { Explainer } from '../shared/Explainer';
import { MobileDynamicBarsRow } from './MobileDynamicBarsRow';
import { ThemePicker } from './ThemePicker';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { PrivatePathsSection } from './PrivatePathsSection';
import { AllowlistEditor } from './AllowlistEditor';
import { StandingDeniesSection } from './StandingDeniesSection';
import { getCcAllowedTools, putCcAllowedTools, getAgentAllowedCommands, putAgentAllowedCommands } from '../../api/client';
import { KeyboardShortcutsSection } from './KeyboardShortcutsSection';
import { MarketplacesSection } from './MarketplacesSection';
import { McpServersPage } from './McpServersPage';
import { WebhooksPage } from './WebhooksPage';
import { EventHooksPage } from './EventHooksPage';
import { GatewayAccessPage, GatewayUpdatesPage, GatewayUsersPage } from './GatewayGroupPages';
import { usePairedDevices, revokePaired } from './pairedDevices';
import { pairedRows, pairingIsKnown } from '../../store/actions/pairedDevices';
import { SectionHeader } from '../shared/SectionHeader';
import { DeviceUserPicker } from './UsersSection';
import {
  RemoteGatewayDevicesSection,
  SettingsGatewaySwitcher,
  ThisGatewayDevicesSection,
  useRemoteGatewayLists,
} from './remoteGatewaySections';
import { remoteGatewaysPaired, settingsGateway } from '../../store/actions/settingsGateway';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import { buildDeviceRows, isUnusedDevice, deviceRowName, deviceRowSummary, submittedDeviceName, type DeviceRowModel } from './deviceList';
import { LocaleSection } from './LocaleSection';
import { CodingAgentBinariesSection } from './CodingAgentBinariesSection';
import { CodingAgentPermissionSection } from './CodingAgentPermissionSection';
import { CodingAgentContextCeilingSection } from './CodingAgentContextCeilingSection';
import { LandingSettingSection } from './LandingSettingSection';
import { MainCheckSettingSection } from './MainCheckSettingSection';
import { CacheKeepAlivePointer, CacheKeepAliveSection } from './CacheKeepAliveSection';
import { IntelligenceLinkRow, IntelligencePage } from './IntelligencePage';
import { SystemPage } from './SystemPage';
import { SystemSubmenu } from './SystemSubmenu';
import { warmSettingsReads } from '../../store/actions/settingsReads';
import { isTauri, describeDeviceUserAgent } from '../../utils/platform';
import { viewportIsMobile } from '../../utils/viewport';
import { CredentialItem } from '../credentials/CredentialItem';
import { OnePasswordSection } from './OnePasswordSection';
import { openAddCredential } from '../../store/actions/credentials';
import { loadRepositories } from '../../store/actions/chat';
import { API, mutatingFetch, throwIfNotOk } from '../../api/client';
import { DirectoryPicker } from './DirectoryPicker';
import { LoadableError } from '../shared/LoadableError';
import { LoadableToggle } from '../shared/LoadableToggle';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { openCommandJudgeModelSettings, openIntelligenceLadder, openSettingsSubview } from '../../store/actions/menu';
import { focusFirstFocusableWithin } from '../layout/paneFocus';
import { formatTimeAgo } from '../../utils/formatTime';
import type { ImageModel } from '../../store/actions/preferences';
import { Disclosure } from '../shared/Disclosure';
import { errorDetail } from '../../utils/errorDetail';
import { motionPreference, scrollBehavior } from '../../utils/motion';
import { themeEffectsPreference } from '../../utils/themeEffects';
import { LADDER_LINK_ROW } from '../chat/rungRows';
import type { PickerLink } from '../shared/ModelSelectionPicker';

const CHAT_LADDER_LINK: PickerLink = {
  option: LADDER_LINK_ROW,
  onChoose: () => openIntelligenceLadder('chat'),
};

function formatScopes(scopes: string): string {
  const map: Record<string, string> = {
    'calendar': 'Calendar',
    'drive': 'Drive',
    'document': 'Docs',
    'spreadsheet': 'Sheets',
    'gmail': 'Gmail',
    'contacts': 'Contacts',
    'userinfo': 'Profile',
  };
  const hidden = new Set(['openid', 'email', 'profile']);
  return scopes
    .split(/[\s,]+/)
    .filter(Boolean)
    .filter((s) => !hidden.has(s))
    .map((s) => {
      const key = s.includes('/') ? s.split('/').pop()! : s;
      for (const [prefix, label] of Object.entries(map)) {
        if (key.toLowerCase().includes(prefix)) return label;
      }
      return key;
    })
    .filter((v, i, a) => a.indexOf(v) === i)
    .join(', ');
}

const THEME_MODE_LABELS: Record<ThemeMode, string> = {
  light: 'Light',
  dark: 'Dark',
  system: 'System',
};

const THEME_MODE_OPTIONS: Array<{ value: ThemeMode; label: string }> = [
  DEFAULT_THEME_MODE,
  ...THEME_MODES.filter((mode) => mode !== DEFAULT_THEME_MODE),
].map((mode) => ({ value: mode, label: THEME_MODE_LABELS[mode] }));

const AGENT_ICON_COLOR_OPTIONS: Array<{ value: AgentIconColors; label: string }> = [
  { value: 'brand', label: 'Brand' },
  { value: 'theme', label: 'Theme' },
  { value: 'muted', label: 'Muted' },
];

const MOTION_LABELS: Record<MotionPref, string> = {
  system: 'System',
  reduce: 'Reduce',
  full: 'Full',
};

const THEME_EFFECTS_LABELS: Record<ThemeEffectsPref, string> = {
  system: 'System',
  reduce: 'Reduce',
  full: 'Full',
};

const EXTERNAL_LINK_TARGET_LABELS: Record<ExternalLinkTarget, string> = {
  safari: 'Safari',
  ask: 'Ask (share sheet)',
  'in-app': 'In-app view',
};

const EXTERNAL_LINK_TARGET_OPTIONS = PREF_EXTERNAL_LINK_TARGET.values
  .map((target) => ({ value: target, label: EXTERNAL_LINK_TARGET_LABELS[target] }));

const MAX_TOOL_CALLS_PRESETS = [50, 100, 250, 1000, 2000, 5000];

const MAX_TOOL_CALLS_OPTIONS = [...new Set([...MAX_TOOL_CALLS_PRESETS, MAX_TOOL_CALLS_DEFAULT])]
  .sort((a, b) => a - b)
  .map((cap) => ({
    value: String(cap),
    label: cap === MAX_TOOL_CALLS_DEFAULT ? `${cap} (default)` : String(cap),
  }));

const IMAGE_MODEL_LABELS: Record<ImageModel, string> = {
  auto: 'Auto',
  'imagen-4': 'Imagen 4',
  'gpt-image-2.5-flare': 'GPT Image 2.5 Flare',
  'gpt-image-2.5-sunburst': 'GPT Image 2.5 Sunburst',
  'gpt-image-2': 'GPT Image 2',
};

const IMAGE_MODEL_CHOICES: ModelChoice[] = PREF_IMAGE_MODEL.values.map((model) => ({
  value: model,
  label: IMAGE_MODEL_LABELS[model],
  reasoningEfforts: [],
}));

function DeviceRow({ row, editingId, setEditingId, onRevoke, gatewayAnswered }: {
  row?: DeviceRowModel;
  editingId?: string | null;
  setEditingId?: (id: string | null) => void;
  onRevoke?: (row: DeviceRowModel) => void;
  gatewayAnswered?: boolean;
}) {
  const sk = useSkeleton();
  const [editValue, setEditValue] = useState('');
  const device = row?.device;
  const isCurrent = !sk && row?.isCurrent === true;
  const displayName = row ? deviceRowName(row) : '';
  const editing = !sk && device != null && editingId === device.id;
  const inputRef = useCallback((el: HTMLInputElement | null) => {
    if (el) { el.focus(); el.select(); }
  }, []);
  const closedRef = useRef(false);

  function startEditing() {
    if (!device) return;
    closedRef.current = false;
    setEditValue(displayName);
    setEditingId?.(device.id);
  }

  function saveEdit() {
    if (!device || closedRef.current) return;
    closedRef.current = true;
    setEditingId?.(null);
    const next = submittedDeviceName(device.name ?? null, displayName, editValue);
    if (next !== undefined) {
      void updateDeviceName(device.id, next);
    }
  }

  function cancelEdit() {
    closedRef.current = true;
    setEditingId?.(null);
  }

  const enableBlocked = !sk && (!device || (!isCurrent && !device.push_enabled));

  return (
    <div class={`list-row ${isCurrent ? 'device-current' : ''}`}>
      <div class="list-row-info">
        <div class="title list-row-name">
          {sk ? (
            <SkText class="device-name" w="9rem" />
          ) : editing ? (
            <input
              class="device-name-input"
              type="text"
              value={editValue}
              onInput={(e) => setEditValue((e.target as HTMLInputElement).value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') saveEdit();
                if (e.key === 'Escape') { e.preventDefault(); cancelEdit(); }
              }}
              onBlur={saveEdit}
              data-escape-self
              ref={inputRef}
            />
          ) : (
            device ? (
              <span class="device-name" onClick={startEditing}>{displayName}</span>
            ) : (
              <span>{displayName}</span>
            )
          )}
          {isCurrent && <span class="device-badge">This device</span>}
        </div>
        <SkText class="list-row-details" as="div" w="14rem">
          {(row
            ? deviceRowSummary(row, describeDeviceUserAgent, gatewayAnswered === true, Date.now())
            : [describeDeviceUserAgent(undefined)]
          ).map((clause) => <span key={clause}>{clause}</span>)}
          {device && (
            <span data-tooltip={formatDateTime(new Date(device.last_seen_at))}>
              {formatTimeAgo(new Date(device.last_seen_at))}
            </span>
          )}
        </SkText>
      </div>
      <div class="list-row-actions">
        {!sk && row?.paired && <DeviceUserPicker deviceId={row.paired.id} />}
        {!sk && (
          <span class={`device-push-label${device?.push_enabled ? '' : ' push-disabled'}`}>Push</span>
        )}
        <SkBlock w="2.25rem" h="1.25rem" round>
          <label
            class={`toggle-switch${enableBlocked ? ' toggle-switch-disabled' : ''}`}
            data-tooltip={enableBlocked ? 'Open Lucidos on that device to enable push' : undefined}
          >
            <input
              type="checkbox"
              checked={device?.push_enabled ?? false}
              disabled={enableBlocked}
              onChange={() => {
                if (device) void setDevicePushEnabled(device.id, !device.push_enabled);
              }}
            />
            <span class="toggle-slider" />
          </label>
        </SkBlock>
        {!sk && row?.paired && onRevoke && (
          <button
            class="action-btn action-btn-danger"
            data-tooltip="Stop this device reaching the machine, on every workspace"
            onClick={() => onRevoke(row)}
          >Revoke</button>
        )}
        {(sk || (device && !isCurrent)) && (
          <SkBlock w="4.5rem" h="2rem" round>
            <button
              class="action-btn action-btn-danger"
              data-tooltip="Forget this device's push and preferences in this workspace"
              onClick={() => { if (device) void removeDevice(device.id); }}
            >Remove</button>
          </SkBlock>
        )}
      </div>
    </div>
  );
}

function AddRepositoryForm() {
  const [adding, setAdding] = useState(false);
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [saving, setSaving] = useState(false);
  const [showPicker, setShowPicker] = useState(false);

  if (!adding) {
    return <ListRowAddCard label="Add Repository" onClick={() => setAdding(true)} />;
  }

  function cancel() {
    setAdding(false);
    setName('');
    setPath('');
  }

  async function save() {
    const trimmedName = name.trim();
    const trimmedPath = path.trim();
    if (!trimmedName || !trimmedPath) return;
    setSaving(true);
    try {
      const res = await mutatingFetch(`${API}/repositories`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: trimmedName, path: trimmedPath }),
      });
      await throwIfNotOk(res);
      cancel();
      void loadRepositories();
    } catch (e) {
      showToast(`Failed to add repository: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  }

  const onFieldKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter') void save();
    if (e.key === 'Escape') cancel();
  };

  return (
    <div class="repo-add-card">
      <input
        class="repo-add-input"
        type="text"
        placeholder="Name"
        value={name}
        onInput={(e) => setName((e.target as HTMLInputElement).value)}
        onKeyDown={onFieldKeyDown}
        autoFocus
      />
      <div class="repo-add-path-row">
        <input
          class="repo-add-input"
          type="text"
          placeholder="Path (e.g. /Users/me/projects/myrepo)"
          value={path}
          onInput={(e) => setPath((e.target as HTMLInputElement).value)}
          onKeyDown={onFieldKeyDown}
        />
        <button class="action-btn repo-add-browse" onClick={() => setShowPicker(true)}>Browse</button>
      </div>
      <div class="repo-add-actions">
        <button class="action-btn" onClick={cancel}>Cancel</button>
        <button class="action-btn action-btn-confirm" disabled={saving || !name.trim() || !path.trim()} onClick={save}>
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
      {showPicker && (
        <DirectoryPicker
          onSelect={(selectedPath) => {
            setPath(selectedPath);
            if (!name.trim()) {
              const lastSeg = selectedPath.split('/').filter(Boolean).pop() || '';
              setName(lastSeg);
            }
            setShowPicker(false);
          }}
          onCancel={() => setShowPicker(false)}
        />
      )}
    </div>
  );
}

function askLucidosToConnectAccount(): void {
  void handleNavigationRequest({
    target: 'new-chat',
    prompt:
      'Help me connect an account: register the app with the provider, '
      + 'enter the client ID, and sign in.',
  });
}

export const SETTINGS_ANCHOR_WAIT_MS = 5000;

function landWhenRendered(anchor: string, land: (el: HTMLElement) => void, missed: () => void): () => void {
  const find = () => document.querySelector<HTMLElement>(`[data-search-anchor="${anchor}"]`);
  const present = find();
  if (present) {
    land(present);
    return () => {};
  }
  const observer = new MutationObserver(() => {
    const el = find();
    if (!el) return;
    stop();
    land(el);
  });
  const timer = setTimeout(() => {
    stop();
    missed();
  }, SETTINGS_ANCHOR_WAIT_MS);
  function stop() {
    observer.disconnect();
    clearTimeout(timer);
  }
  observer.observe(document.body, { childList: true, subtree: true });
  return stop;
}

export function SettingsView() {
  const loadable = devices.value;
  const showLoading = useDelayedLoading(loadable);
  const paired = usePairedDevices();
  useRemoteGatewayLists(settingsSubview.value === 'devices' || settingsSubview.value === 'users');
  const background = useBackgroundModels();
  usePanelRefresh('background models', background.refresh);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [changingAgentSandbox, setChangingAgentSandbox] = useState(false);
  const [unusedDevicesOpen, setUnusedDevicesOpen] = useState(false);
  const [oauthProvider, setOauthProvider] = useState('');
  const [oauthConnecting, setOauthConnecting] = useState(false);
  const [oauthPurpose, setOauthPurpose] = useState<string | null>(null);
  const registryLoadable = knownOAuthProviders.value;
  const registryProviders =
    registryLoadable.status === 'loaded' ? registryLoadable.data.providers : [];
  preferences.value;
  const uiScale = currentUiScale();
  const styleOverrideCount = Object.keys(currentStyleOverrides()).length;
  const imageModel = currentImageModel();

  useEffect(warmSettingsReads, []);
  usePanelRefresh(`${settingsSubview.value} settings`, SETTINGS_SECTION_REFRESH[settingsSubview.value] ?? null);

  useEffect(() => {
    if (repositories.value.status === 'not-loaded') {
      void loadRepositories();
    }
  }, [repositories.value.status]);

  const anchorWatch = useRef<(() => void) | null>(null);
  useEffect(() => () => anchorWatch.current?.(), [settingsSubview.value]);

  useEffect(() => {
    const view = settingsSubview.value;
    const fragment = settingsLinkedFragment.value;
    const target = settingsScrollTarget.value ?? (fragment && settingsLinkAnchor(view, fragment));
    if (!target || view === 'main') return;
    settingsLinkedFragment.value = null;
    settingsScrollTarget.value = null;
    anchorWatch.current?.();
    anchorWatch.current = landWhenRendered(aliasRetiredSettingsAnchor(target), (el) => {
      el.scrollIntoView({ block: 'center', behavior: scrollBehavior() });
      const markEl = (el.closest('.settings-row') as HTMLElement | null) ?? el;
      applyNavFocus(markEl);
      focusFirstFocusableWithin(markEl);
    }, () => {
      if (fragment) showToast(`No "${fragment}" on ${settingsSubviewLabel(view) ?? view} settings`, 'error');
    });
  }, [settingsScrollTarget.value, settingsLinkedFragment.value, settingsSubview.value]);

  useEffect(() => {
    if (settingsSubview.value !== 'accounts') return;
    if (knownOAuthProviders.value.status !== 'not-loaded') return;
    void loadKnownOAuthProviders();
  }, [settingsSubview.value, knownOAuthProviders.value.status]);

  useEffect(() => {
    const prefill = oauthConnectPrefill.value;
    if (!prefill || settingsSubview.value !== 'accounts') return;
    if (knownOAuthProviders.value.status === 'loading') return;
    setOauthProvider(prefillLabel(registryProviders, prefill.provider));
    setOauthPurpose(prefill.scopes ?? null);
    oauthConnectPrefill.value = null;
  }, [oauthConnectPrefill.value, settingsSubview.value, knownOAuthProviders.value.status]);

  function credentialsSection() {
    const credLoadable = credentials.value;
    if (credLoadable.status === 'failed') {
      return <div class="list-rows"><LoadableError noun="credentials" error={credLoadable.error} /></div>;
    }
    if (credLoadable.status !== 'loaded') {
      return null;
    }
    return (
      <div class="list-rows">
        {credLoadable.data
          .filter((cred) => cred.auth_type !== 'onepassword_service_account')
          .map((cred) => (
            <CredentialItem key={cred.id} credential={cred} />
          ))}
        <ListRowAddCard label="Add Credential" onClick={openAddCredential} />
      </div>
    );
  }

  async function handleConnectProvider() {
    if (!oauthProvider.trim()) return;
    const provider = providerToSend(registryProviders, oauthProvider);
    if (!provider) return;
    setOauthConnecting(true);
    try {
      await grantOAuthScope(provider, oauthPurpose?.trim() ?? '', true);
      setOauthProvider('');
      setOauthPurpose(null);
    } finally {
      setOauthConnecting(false);
    }
  }

  function oauthSection() {
    const oauthLoadable = oauthAccounts.value;
    if (oauthLoadable.status === 'failed') {
      return <div class="list-rows"><LoadableError noun="accounts" error={oauthLoadable.error} /></div>;
    }
    if (oauthLoadable.status !== 'loaded') {
      return null;
    }
    return (
      <div class="list-rows">
        {oauthLoadable.data.map(account => {
          const scopeLabel = account.scopes ? formatScopes(account.scopes) : '';
          const shortfall = missingScopes(account);
          const consoleRow = reauthorizationHint(
            registryProviders,
            account.provider,
            shortfall.length > 0,
          );
          return (
          <div class="list-row oauth-account-row" key={account.id}>
            <div class="list-row-info">
              <div class="title list-row-name">{account.provider}</div>
              <div class="list-row-details">
                <span>{account.email || 'No email'}</span>
                {scopeLabel && <span>{scopeLabel}</span>}
              </div>
              {shortfall.length > 0 && (
                <div class="oauth-account-shortfall">
                  Missing {shortfall.join(', ')}. The provider refused{' '}
                  {shortfall.length === 1 ? 'it' : 'them'}: enable{' '}
                  {shortfall.length === 1 ? 'it' : 'them'} for your app with the provider,
                  then Reconnect.
                </div>
              )}
              {consoleRow && <ProviderPermissionsHint row={consoleRow} />}
              <div class="list-row-date">
                <span data-tooltip={formatDateTime(new Date(account.created_at))}>
                  Connected {formatShortDateWithYear(new Date(account.created_at))}
                </span>
              </div>
            </div>
            <div class="list-row-actions">
              <button
                class="action-btn"
                onClick={() => void grantOAuthScope(account.provider, reconnectScopes(account))}
              >Reconnect</button>
              <button
                class="action-btn action-btn-danger"
                onClick={() => void disconnectOAuthAccount(account.id, account.provider)}
              >Disconnect</button>
            </div>
          </div>
          );
        })}
        <div class="list-row oauth-connect-row">
          <div class="oauth-quick-providers">
            {registryProviders.map(p => (
              <button
                key={p.id}
                class={`oauth-quick-btn${oauthProvider.toLowerCase() === p.label.toLowerCase() ? ' active' : ''}`}
                disabled={oauthConnecting}
                onClick={() => {
                  setOauthPurpose(null);
                  setOauthProvider(prev =>
                    prev.toLowerCase() === p.label.toLowerCase() ? '' : p.label);
                }}
              >{p.label}</button>
            ))}
          </div>
          <div class="oauth-connect-controls">
            <input
              class="oauth-provider-input"
              type="text"
              placeholder="or type a provider name"
              value={oauthProvider}
              disabled={oauthConnecting}
              onInput={(e) => setOauthProvider((e.target as HTMLInputElement).value)}
              onKeyDown={(e) => { if (e.key === 'Enter') void handleConnectProvider(); }}
            />
            <button
              class="action-btn action-btn-confirm"
              disabled={oauthConnecting || !oauthProvider.trim()}
              onClick={handleConnectProvider}
            >{oauthConnecting ? 'Connecting...' : 'Connect'}</button>
          </div>
          <p class="form-hint oauth-connect-hint">
            Connecting needs an app registration with the provider.{' '}
            <button class="accent-link" onClick={askLucidosToConnectAccount}>
              Ask Lucidos to do it
            </button>{' '}
            if you would rather be walked through it.
          </p>
        </div>
      </div>
    );
  }

  function accountsSection() {
    return (
      <>
        <div class="settings-section accounts-section">
          <div class="settings-section-title" data-search-anchor="accounts:connected">
            Connected accounts
            <Explainer title="Connected accounts">
              <p>Services you have signed in to, so Lucidos can act on your behalf.</p>
              <p>
                Signing in happens here, in your browser, and Lucidos keeps the
                resulting access.
              </p>
            </Explainer>
          </div>
          {oauthSection()}
        </div>
        <div class="settings-section accounts-section">
          <div class="settings-section-title" data-search-anchor="accounts:credentials">
            Credentials
            <Explainer title="Credentials">
              <p>
                Secrets Lucidos stores for you: API keys, tokens and passwords, and the
                OAuth app registrations behind the accounts above.
              </p>
            </Explainer>
          </div>
          {credentialsSection()}
        </div>
        <OnePasswordSection />
      </>
    );
  }

  function devicesSection() {
    if (loadable.status === 'failed') {
      return (
        <div class="list-rows">
          <LoadableError noun="devices" error={loadable.error} />
        </div>
      );
    }
    const rowsPaired = pairedRows(paired.paired);
    function revoke(row: DeviceRowModel) {
      if (!row.paired) return;
      void revokePaired(row.paired, {
        name: deviceRowName(row),
        selfId: paired.selfId,
        count: rowsPaired?.length ?? 0,
        reload: paired.reload,
      });
    }
    return (
      <div class="list-rows" data-search-anchor="devices:list">
        {paired.paired.status === 'failed' && (
          <LoadableError noun="paired devices" error={paired.paired.error} />
        )}
        <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf containerClass="list-rows" row={() => <DeviceRow />} />}>
          {loadable.status === 'loaded'
            ? (() => {
                const rows = buildDeviceRows(loadable.data, rowsPaired, getDeviceId());
                if (rows.length === 0) {
                  return <div class="empty-state">No devices registered</div>;
                }
                const unused = rows.filter(isUnusedDevice);
                const drawRow = (row: DeviceRowModel) => (
                  <DeviceRow
                    key={row.id}
                    row={row}
                    editingId={editingId}
                    setEditingId={setEditingId}
                    onRevoke={revoke}
                    gatewayAnswered={pairingIsKnown(paired.paired)}
                  />
                );
                return (
                  <>
                    {rows.filter((row) => !isUnusedDevice(row)).map(drawRow)}
                    {unused.length > 0 && (
                      <>
                        <SectionHeader
                          title="Unused devices"
                          count={unused.length}
                          collapsed={!unusedDevicesOpen}
                          onToggle={() => setUnusedDevicesOpen((open) => !open)}
                        />
                        <Disclosure open={unusedDevicesOpen}>{unused.map(drawRow)}</Disclosure>
                      </>
                    )}
                  </>
                );
              })()
            : null}
        </LoadingFade>
      </div>
    );
  }

  function devicesPage() {
    const gateway = settingsGateway.value;
    if (gateway === REMOTE_GATEWAY && gateway === null) {
      if (!remoteGatewaysPaired.value) return devicesSection();
      return (
        <div class="settings-section">
          <div class="settings-section-title">On this gateway</div>
          {devicesSection()}
        </div>
      );
    }
    if (gateway === null) return <ThisGatewayDevicesSection />;
    return (
      <>
        <RemoteGatewayDevicesSection gateway={gateway} />
        {gateway === REMOTE_GATEWAY && (
          <div class="settings-section">
            <div class="settings-section-title">In this workspace</div>
            {devicesSection()}
          </div>
        )}
      </>
    );
  }

  function repositoriesSection() {
    const repoLoadable = repositories.value;

    const rows =
      repoLoadable.status === 'failed'
        ? <LoadableError noun="repositories" error={repoLoadable.error} />
        : repoLoadable.status !== 'loaded'
          ? null
          : (
            <>
              {repoLoadable.data.map(repo => (
                <div class="list-row" key={repo.id}>
                  <div class="list-row-info">
                    <div class="title">{repo.name}</div>
                    <div class="list-row-details">{repo.path}</div>
                  </div>
                  <div class="list-row-actions">
                    <button class="action-btn action-btn-danger" onClick={async () => {
                      if (await showConfirm(`Remove "${repo.name}"?`, 'Remove', { variant: 'danger' })) {
                        try {
                          const res = await mutatingFetch(`${API}/repositories/${repo.id}`, { method: 'DELETE' });
                          await throwIfNotOk(res);
                          void loadRepositories();
                        } catch (e) {
                          showToast(`Failed to remove repository: ${errorDetail(e)}`, 'error');
                        }
                      }
                    }}>Remove</button>
                  </div>
                </div>
              ))}
              <AddRepositoryForm />
            </>
          );

    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="coding-agents:repositories">
          Repositories
          <Explainer title="Repositories">
            <p>Register local git repositories for Claude Code sessions.</p>
          </Explainer>
        </div>
        <div class="list-rows">{rows}</div>
      </div>
    );
  }

  function codingAgentsSection() {
    return (
      <>
        <CodingAgentBinariesSection />
        <CodingAgentPermissionSection />
        <CodingAgentContextCeilingSection />
        <LandingSettingSection />
        <MainCheckSettingSection />
        <CacheKeepAlivePointer />
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="coding-agents:intelligence">Intelligence ladders</div>
          <IntelligenceLinkRow>Which model and effort an agent may pick for Claude Code and Codex</IntelligenceLinkRow>
        </div>
        {repositoriesSection()}
      </>
    );
  }

  function commandSafetySection() {
    const loaded = preferences.value.status === 'loaded';
    const guardOn = currentCommandGuard();
    const judgeOn = currentCommandGuardJudge();
    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="command-safety">
          Command safety
          <Explainer title="Command safety">
            <p>
              The command guard gates the <strong>Lucidos Agent's</strong> own shell and
              Python tools, in chat and in triggers. It classifies each command before it
              runs. In chat a risky one asks you first. A trigger has nobody to ask, so it
              runs against the side-effect grant it declared, and fails when that grant
              does not cover the command. With the guard off it classifies nothing, and
              every command the agent runs goes straight through.
            </p>
            <p>
              The guard does not answer a coding agent's cards. Claude Code asks through
              its own permissions list below. Codex asks when a command has to escape its
              sandbox. Auto-answer coding-agent cards, under Coding agent permissions,
              hands those to Lucidos. Lucidos answers the plain reads for you, whatever
              these toggles say.
            </p>
            <p>
              The <strong>LLM judge</strong> places the commands the static check cannot.
              It answers a coding agent only under auto-answer.
            </p>
            <p>
              The judge never runs on your chat model. With no judge model a provider
              serves, a Lucidos Agent turn refuses to start and says why. Pick one under
              Models, or turn the judge off to let the static check decide alone.
            </p>
          </Explainer>
        </div>
        <div class="settings-row" data-search-anchor="command-safety:guard">
          <span class="settings-row-label">Command guard</span>
          <LoadableToggle
            loaded={loaded}
            checked={guardOn}
            onChange={(c) => void setCommandGuard(c)}
          />
        </div>
        <div class="settings-row settings-row-child" data-search-anchor="command-safety:judge">
          <span class="settings-row-label">LLM judge</span>
          <LoadableToggle
            loaded={loaded}
            checked={judgeOn}
            disabled={!guardOn}
            onChange={(c) => void setCommandGuardJudge(c)}
          />
        </div>
        {commandJudgeLinkRow()}
      </div>
    );
  }

  function commandJudgeLinkRow() {
    return (
      <div class="settings-row settings-row-child">
        <span class="settings-row-label">Judge model</span>
        <button type="button" class="action-btn" onClick={openCommandJudgeModelSettings}>
          Choose in Models
        </button>
      </div>
    );
  }

  function codingAgentAutoAnswerRow() {
    return (
      <div class="settings-row" data-search-anchor="permissions:coding-agent-auto-answer">
        <span class="settings-row-label">
          Auto-answer coding-agent cards
          <Explainer title="Auto-answer coding-agent cards">
            <p>
              When <strong>Claude Code</strong> or <strong>Codex</strong> asks to run a
              command, Lucidos can answer for you. It uses the command guard's checks and
              its judge. A command placed as safe or reversible runs. An irreversible or
              catastrophic one never runs unasked.
            </p>
            <p>
              It sees only what would otherwise ask you. The Claude Code permission mode
              and the list below settle the rest first.
            </p>
            <p>
              Nothing takes a checkpoint first, so a wrong reversible call has no undo.
            </p>
            <p>
              Off, Lucidos runs no Claude Code command for you, so what Claude Code asks
              reaches you. The command guard switch does not change this.
            </p>
            <p>
              Whatever these switches say, Lucidos refuses a catastrophic command it
              recognises, such as wiping a disk, without asking.
            </p>
            <p>
              <strong>Codex is different.</strong> It asks only when its own sandbox
              blocked a command, such as <code>ps</code> on macOS. You never chose that
              block, so Lucidos lets a safe one run whatever these switches say.
            </p>
          </Explainer>
        </span>
        <LoadableToggle
          loaded={preferences.value.status === 'loaded'}
          checked={currentCodingAgentAutoAnswer()}
          onChange={(c) => void setCodingAgentAutoAnswer(c)}
        />
      </div>
    );
  }

  async function changeAgentsUnsandboxed(on: boolean) {
    if (on) {
      const ok = await showConfirm(
        'Agents will run without the sandbox. Any command, script or coding agent can then read Lucidos\'s own secrets on this machine. These include its sign-in tokens and database password.\n\nTurn Sandbox agents off only where the sandbox cannot run and you accept that.',
        'Run unsandboxed',
        { variant: 'default' },
      );
      if (!ok) return;
    }
    await setAgentsUnsandboxed(on);
  }

  function agentSandboxSection() {
    const loaded = preferences.value.status === 'loaded';
    const unavailable = agentSandboxUnavailable.value;
    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="agent-sandbox">
          Agent sandbox
          <Explainer title="Agent sandbox">
            <p>
              With Sandbox agents on, an OS sandbox protects every process an agent runs.
              This covers the Lucidos Agent's commands and scripts, triggers, coding
              agents, background tasks and MCP servers. The sandbox hides Lucidos's own secrets, such as its sign-in tokens
              and database password, and keeps each workspace's permission lists read-only.
              The rest of your files stay as reachable as before.
            </p>
            <p>
              Where this machine cannot apply the sandbox, those processes do not start.
              Turn Sandbox agents off to let them start without that protection.
            </p>
          </Explainer>
        </div>
        {unavailable !== null && (
          <div class="settings-row">
            <span class="settings-row-label">
              This machine cannot sandbox agents: {unavailable}
            </span>
          </div>
        )}
        {agentSandboxTruncationUnguarded.value && (
          <div class="settings-row">
            <span class="settings-row-label">
              On this system the sandbox cannot stop an agent from emptying a protected
              file. It still stops reading, changing and replacing one. Linux 6.2 or later
              closes this gap.
            </span>
          </div>
        )}
        {agentSandboxMetadataUnguarded.value && (
          <div class="settings-row">
            <span class="settings-row-label">
              On this system the sandbox cannot stop a trigger from changing the permissions,
              owner or timestamps of a file outside its folders. It still stops changing the
              file's contents. Allowing unprivileged user namespaces closes this gap.
            </span>
          </div>
        )}
        <div class="settings-row" data-search-anchor="agent-sandbox:unsandboxed">
          <span class="settings-row-label">Sandbox agents</span>
          <LoadableToggle
            loaded={loaded}
            checked={!currentAgentsUnsandboxed()}
            ariaLabel="Sandbox agents"
            disabled={changingAgentSandbox}
            onChange={(c) => {
              setChangingAgentSandbox(true);
              changeAgentsUnsandboxed(!c).catch((e: unknown) => {
                showToast(`Could not change the agent sandbox: ${e instanceof Error ? e.message : String(e)}`, 'error');
              }).finally(() => setChangingAgentSandbox(false));
            }}
          />
        </div>
      </div>
    );
  }

  function permissionsSection() {
    return (
      <>
        {agentSandboxSection()}
        <PrivatePathsSection />
        {commandSafetySection()}
        <StandingDeniesSection />
        <AllowlistEditor
          title="Lucidos Agent permissions"
          anchor="permissions:lucidos"
          noun="command permissions"
          rememberButton="Always allow"
          placeholder="Bash(git:*)"
          load={getAgentAllowedCommands}
          save={putAgentAllowedCommands}
          description={
            <>
              Commands the Lucidos Agent may run without asking when the command guard is on.
              Patterns: <code>Bash(&lt;first-token&gt;:*)</code> e.g. <code>Bash(git:*)</code>, <code>Bash</code> (any shell), <code>Python</code> (any Python).
              Use the <strong>Always allow</strong> buttons on a command prompt to add entries quickly. Edits apply to the next command — no restart.
            </>
          }
        />
        <AllowlistEditor
          title="Coding agent permissions"
          anchor="permissions:coding-agent"
          noun="tool permissions"
          rememberButton="Always allow"
          placeholder="Bash(npm:*)"
          load={getCcAllowedTools}
          save={putCcAllowedTools}
          lead={(
            <>
              {codingAgentAutoAnswerRow()}
              {commandJudgeLinkRow()}
            </>
          )}
          description={
            <>
              Tools and commands coding agents may run without asking. Claude Code reads every line, Codex the command lines: <code>Bash(git:*)</code> covers <code>git</code> for both.
              Use the <strong>Always allow</strong> buttons on permission prompts to add entries quickly. A line naming a shell such as <code>Bash(bash:*)</code> is ignored, since it would cover any command.
              Claude Code reads changes in new sessions, Codex on its next prompt.
            </>
          }
        />
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="permissions:mcp">
            MCP tool permissions
          </div>
          <div class="settings-row">
            <span class="settings-row-label">
              MCP servers
              <Explainer title="MCP tool permissions">
                <p>
                  The MCP allowlist lives beside the server list, on its own page. A
                  pattern there is meaningless without knowing which servers are
                  registered and which tools each one offers.
                </p>
                <p>
                  That page also says what every server costs each request, and lets you
                  switch a server or a single tool off.
                </p>
              </Explainer>
            </span>
            <button class="action-btn" onClick={() => openSettingsSubview('mcp')}>
              Open
            </button>
          </div>
        </div>
      </>
    );
  }

  function modelsSection() {
    const maxToolCalls = currentMaxToolCalls();
    const triggerModel = currentTriggerModel();

    function handleMaxToolCallsChange(raw: string) {
      const trimmed = raw.trim();
      if (!/^\d+$/.test(trimmed)) {
        showToast(`"${raw}" is not a whole number of tool calls`, 'error');
        return;
      }
      const parsed = Number(trimmed);
      if (parsed > MAX_TOOL_CALLS_MAX) {
        showToast(
          `${trimmed} is above the limit of ${MAX_TOOL_CALLS_MAX.toLocaleString()} tool calls`,
          'error',
        );
        return;
      }
      const next = Math.max(MAX_TOOL_CALLS_MIN, parsed);
      if (next === maxToolCalls) return;
      void setMaxToolCalls(next);
    }

    return (
      <>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:chat">
            Chat &amp; triggers
            <Explainer title="Chat &amp; triggers">
              <p>
                Chat model is the default for new chat threads. Trigger model is the
                default for every trigger that pins none, so a costly chat model
                does not run every unattended fire.
              </p>
              <p>
                A trigger can pin its own instead, on its edit form in the Triggers
                panel.
              </p>
            </Explainer>
          </div>
          <ModelSelectionRow
            label="Chat model"
            anchor="models:chat-model"
            models={lucidosModelChoices(currentModel.value)}
            vocabulary={LUCIDOS_TIER_VOCABULARY}
            model={currentModel.value}
            effort={accountChatEffort(currentModel.value)}
            effortFor={accountChatEffort}
            unsetEffortLabel={PROVIDER_DEFAULT_EFFORT_LABEL}
            defaultEffortLabel={(m) => defaultTierLabel(modelDefaultEffort(m))}
            link={CHAT_LADDER_LINK}
            onChange={(p) => {
              void setChatModelSelection(p);
              if (p.provider) void rememberProvider(p.model, p.provider);
            }}
          />
          <ModelSelectionRow
            label="Trigger model"
            anchor="models:trigger-model"
            models={lucidosModelChoices(triggerModel)}
            vocabulary={LUCIDOS_TIER_VOCABULARY}
            model={triggerModel}
            effort={accountChatEffort(triggerModel)}
            effortFor={accountChatEffort}
            unsetEffortLabel={PROVIDER_DEFAULT_EFFORT_LABEL}
            defaultEffortLabel={(m) => defaultTierLabel(modelDefaultEffort(m))}
            link={CHAT_LADDER_LINK}
            onChange={(p) => {
              void setTriggerModelSelection(p);
              if (p.provider) void rememberProvider(p.model, p.provider);
            }}
          />
          <div class="settings-row" data-search-anchor="models:max-tool-calls">
            <span class="settings-row-label">
              Max tool calls
              <Explainer title="Max tool calls">
                <p>
                  How many tool calls the agent may make in a single turn before the
                  engine stops it. Applies to chat and triggers alike.
                </p>
                <p>
                  Hitting the limit is not an error: the turn ends with a message you
                  can continue from by sending anything.
                </p>
                <p>
                  The cap is what bounds a runaway turn, and it's roughly how long
                  one can run. Cost grows faster than time, because every step
                  resends the conversation.
                </p>
              </Explainer>
            </span>
            <Dropdown
              options={MAX_TOOL_CALLS_OPTIONS}
              value={String(maxToolCalls)}
              freeText
              placeholder={`e.g. ${MAX_TOOL_CALLS_DEFAULT}`}
              onChange={handleMaxToolCallsChange}
            />
          </div>
          <div class="settings-row-note">
            {maxToolCalls.toLocaleString()} allows up to about{' '}
            {estimateTurnDuration(maxToolCalls)} of work.
          </div>
        </div>
        <ResponseStylesSection />
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:image-generation">Image generation</div>
          <ModelSelectionRow
            label="Model"
            models={IMAGE_MODEL_CHOICES}
            vocabulary={LUCIDOS_TIER_VOCABULARY}
            model={imageModel}
            effort={null}
            onChange={(p) => void setImageModel(p.model as ImageModel)}
          />
        </div>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:background-tasks">Background tasks</div>
          <BackgroundModelRow
            label="Title generation"
            anchor="models:title-generation"
            modelKey="model_title"
            reasoningKey="reasoning_title"
            background={background}
          />
          <TitleStyleRow />
          <BackgroundModelRow
            label="Change summary"
            anchor="models:change-summary"
            nested
            modelKey="model_change_summary"
            reasoningKey="reasoning_change_summary"
            background={background}
          />
          <BackgroundModelRow
            label="Image description"
            anchor="models:image-description"
            modelKey="model_image_description"
            reasoningKey="reasoning_image_description"
            background={background}
          />
          <BackgroundModelRow
            label="Memory extraction"
            anchor="models:memory-extraction"
            modelKey="model_memory"
            reasoningKey="reasoning_memory"
            background={background}
          />
          <JudgmentModelRow
            site="query-classification"
            label="Query classification"
            anchor="models:query-classification"
            nested
            background={background}
          />
          <BackgroundModelRow
            label="Conversation summary"
            anchor="models:conversation-summary"
            modelKey="model_conversation_summary"
            reasoningKey="reasoning_conversation_summary"
            background={background}
          />
          <JudgmentModelRow
            site="command-guard"
            label="Command judge"
            anchor="models:command-judge"
            background={background}
          />
          <JudgmentModelRow
            site="read-decision"
            label="Read decision"
            anchor="models:read-decision"
            background={background}
          />
          <JudgmentModelRow
            site="wake-decision"
            label="Wake decision"
            anchor="models:wake-decision"
            nested
            background={background}
          />
          <JudgmentModelRow
            site="expected-wake-decision"
            label="Expected wake"
            anchor="models:expected-wake-decision"
            nested
            background={background}
          />
          <JudgmentModelRow
            site="permission-answer"
            label="Permission answer"
            anchor="models:permission-answer"
            background={background}
          />
          <Disclosure open={currentMemoryModule() === 'tree'}>
            <BackgroundModelRow
              label="Summary compaction"
              anchor="models:summary-compaction"
              modelKey="model_summary_compaction"
              reasoningKey="reasoning_summary_compaction"
              background={background}
            />
          </Disclosure>
        </div>
        <CacheKeepAliveSection />
        <VoiceSection />
        <ModelsManager />
      </>
    );
  }

  function appearanceSection() {
    const mode = currentThemeMode();
    const motion = motionPreference.value;
    const themeEffects = themeEffectsPreference.value;
    const font = currentFontFamily();
    const iconColors = currentAgentIconColors();

    return (
      <>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="appearance:theme">
            Theme
            <Explainer title="Theme">
              <p>
                A theme retunes the colours, the headers and how Lucidos shows
                focus, on this device. Some themes style both light and dark,
                some only one. Mode picks which one you see.
              </p>
              <p>
                Plugins can add themes, and so can the agent: ask it for a
                theme like the one you have in mind.
              </p>
            </Explainer>
          </div>
          <div class="settings-row" data-search-anchor="appearance:mode">
            <span class="settings-row-label">Mode</span>
            <div class="segmented-control" role="group" aria-label="Theme mode">
              {THEME_MODE_OPTIONS.map((t) => (
                <button
                  key={t.value}
                  type="button"
                  aria-pressed={mode === t.value}
                  class={`segmented-btn ${mode === t.value ? 'active' : ''}`}
                  onClick={() => void setThemeMode(t.value)}
                >
                  {t.label}
                </button>
              ))}
            </div>
          </div>
          <ThemePicker />
          <div class="settings-row" data-search-anchor="appearance:agent-icon-colors">
            <span class="settings-row-label">
              Agent icon colors
              <Explainer title="Agent icon colors">
                <p>
                  How the Claude, Codex and Lucidos icons look in the thread
                  drawer, the thread pane and navigation history on this
                  device.
                </p>
                <p>
                  Brand draws each agent in its own colors. Theme draws them
                  all in the theme's accent color, and Muted in its muted text
                  color. A theme can tune all three.
                </p>
              </Explainer>
            </span>
            <div class="segmented-control" role="group" aria-label="Agent icon colors">
              {AGENT_ICON_COLOR_OPTIONS.map(option => (
                <button
                  key={option.value}
                  type="button"
                  aria-pressed={iconColors === option.value}
                  class={`segmented-btn ${iconColors === option.value ? 'active' : ''}`}
                  onClick={() => void setAgentIconColors(option.value)}
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>
          <div class="settings-row" data-search-anchor="appearance:theme-effects">
            <span class="settings-row-label">
              Effects
              <Explainer title="Effects">
                <p>
                  Whether the glows, shadows and scanlines a theme adds show on
                  this device. System drops them when your device asks for more
                  contrast or less transparency.
                </p>
                <p>
                  Reduce keeps the theme's colours, letter spacing and borders
                  and drops its glows and scanlines, which saves battery and
                  sharpens text. Full shows every effect.
                </p>
              </Explainer>
            </span>
            <div class="segmented-control" role="group" aria-label="Theme effects">
              {THEME_EFFECTS_PREFS.map((e) => (
                <button
                  key={e}
                  type="button"
                  aria-pressed={themeEffects === e}
                  class={`segmented-btn ${themeEffects === e ? 'active' : ''}`}
                  onClick={() => void setThemeEffects(e)}
                >
                  {THEME_EFFECTS_LABELS[e]}
                </button>
              ))}
            </div>
          </div>
        </div>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="appearance:motion">Motion</div>
          <div class="settings-row">
            <span class="settings-row-label">
              Animation
              <Explainer title="Motion">
                <p>
                  How much Lucidos moves on this device. System follows your
                  device's reduce-motion setting.
                </p>
                <p>
                  Reduce keeps the app calm: no slides, pulses or spinning, and
                  changes appear at once. Full keeps every animation, even when
                  your device asks for less.
                </p>
              </Explainer>
            </span>
            <div class="segmented-control" role="group" aria-label="Motion">
              {MOTION_PREFS.map((m) => (
                <button
                  key={m}
                  type="button"
                  aria-pressed={motion === m}
                  class={`segmented-btn ${motion === m ? 'active' : ''}`}
                  onClick={() => void setMotion(m)}
                >
                  {MOTION_LABELS[m]}
                </button>
              ))}
            </div>
          </div>
        </div>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="appearance:typography">Typography</div>
          <div class="settings-row" data-search-anchor="appearance:font">
            <span class="settings-row-label">Font</span>
            <Dropdown
              options={workspaceFontList.value.status === 'loaded'
                ? fontOptions(workspaceFontList.value.data.fonts, font)
                : fontOptions([])}
              value={font}
              onChange={(v) => void setFontFamily(v as FontPreference)}
            />
          </div>
          <WorkspaceFontsSection />
          <div class="settings-row" data-search-anchor="appearance:ui-scale">
            <span class="settings-row-label">UI scale</span>
            <button class="settings-option" onClick={openScaleModal}>
              {uiScale}%
            </button>
          </div>
        </div>
        {styleOverrideCount > 0 && (
          <div class="settings-section">
            <div class="settings-section-title" data-search-anchor="appearance:style-overrides">Style overrides</div>
            <div class="settings-row" data-search-anchor="appearance:style-overrides-clear">
              <span class="settings-row-label">
                {styleOverrideCount} {styleOverrideCount === 1 ? 'value' : 'values'} tuned by the style remote
              </span>
              <button
                class="action-btn action-btn-danger"
                onClick={() => void clearStyleOverrides()}
              >
                Clear all
              </button>
            </div>
          </div>
        )}
        {viewportIsMobile.value && (
          <div class="settings-section">
            <div class="settings-section-title" data-search-anchor="appearance:mobile">Mobile</div>
            <MobileDynamicBarsRow />
          </div>
        )}
        {notificationsSection()}
        {linksSection()}
      </>
    );
  }

  function notificationsSection() {
    const currentId = getDeviceId();
    const current = loadable.status === 'loaded'
      ? loadable.data.find((d) => d.id === currentId)
      : undefined;
    const unregistered = loadable.status === 'loaded' && current === undefined;
    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="appearance:notifications">
          Notifications
        </div>
        {loadable.status === 'failed' ? (
          <LoadableError noun="devices" error={loadable.error} />
        ) : (
          <>
            <div class="settings-row" data-search-anchor="appearance:push-notifications">
              <span class="settings-row-label">
                Push notifications
                <Explainer title="Push notifications">
                  {(close) => (
                    <>
                      <p>
                        Whether this device gets a notification outside Lucidos when
                        something needs you.
                      </p>
                      <p>
                        Every device has its own switch, and the rest live under{' '}
                        <button
                          class="accent-link"
                          onClick={() => {
                            close();
                            openSettingsSubview('devices');
                          }}
                        >
                          Devices
                        </button>.
                      </p>
                    </>
                  )}
                </Explainer>
              </span>
              <LoadableToggle
                loaded={current !== undefined}
                checked={current?.push_enabled ?? false}
                onChange={(c) => void setDevicePushEnabled(currentId, c)}
              />
            </div>
            {unregistered && (
              <div class="settings-row-note">
                This device has not registered with the engine yet, so there is nothing
                to switch on. Reload the page to try again.
              </div>
            )}
          </>
        )}
        <div class="settings-row" data-search-anchor="appearance:in-app-toasts">
          <span class="settings-row-label">
            In-app toasts
            <Explainer title="In-app toasts">
              <p>
                The pop-up that appears over Lucidos when a notification arrives
                while you are using it.
              </p>
              <p>
                Turn it off and the notification goes straight to the bell and the
                Notifications panel, so you find it when you are ready. No push
                arrives instead: a device you are already using never gets one.
              </p>
              <p>
                Unlike the switch above, this one covers every device.
              </p>
            </Explainer>
          </span>
          <LoadableToggle
            loaded={preferences.value.status === 'loaded'}
            checked={currentNotificationToasts()}
            onChange={(c) => void setNotificationToasts(c)}
          />
        </div>
      </div>
    );
  }

  function linksSection() {
    const showExternalTarget = externalLinkTargetConfigurable();
    const showInAppBrowser = isTauri();
    if (!showExternalTarget && !showInAppBrowser) return null;
    const loaded = preferences.value.status === 'loaded';
    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="appearance:links">Links</div>
        {showExternalTarget && (
          <div class="settings-row" data-search-anchor="appearance:external-link-target">
            <span class="settings-row-label">
              Open links in
              <Explainer title="Open links in">
                <p>
                  Where a link to another site goes when you tap it. Installed on the
                  home screen, iOS would otherwise trap every link in an in-app view
                  with no address bar and no shared Safari session.
                </p>
                <p>
                  iOS gives web apps no way to reach your default-browser setting
                  directly, so "Ask" is the option that lets iOS itself offer every
                  browser you have installed.
                </p>
                <p>
                  For a one-off, long-press any link instead: iOS offers Open in
                  Safari, Copy and Share without changing this setting.
                </p>
              </Explainer>
            </span>
            <Dropdown
              options={EXTERNAL_LINK_TARGET_OPTIONS}
              value={currentExternalLinkTarget()}
              onChange={(v) => void setExternalLinkTarget(v as ExternalLinkTarget)}
            />
          </div>
        )}
        {showInAppBrowser && (
          <div class="settings-row" data-search-anchor="appearance:in-app-browser">
            <span class="settings-row-label">
              Open links in the in-app browser
              <Explainer title="Open links in the in-app browser">
                <p>
                  Open links inside the app in a built-in browser pane instead of
                  your system browser, and add a Browser entry to the menu drawer.
                </p>
                <p>
                  Experimental: the in-app browser has rough edges, so it's off by
                  default.
                </p>
              </Explainer>
            </span>
            <LoadableToggle
              loaded={loaded}
              checked={currentInAppBrowser()}
              onChange={(c) => void setInAppBrowser(c)}
            />
          </div>
        )}
      </div>
    );
  }

  function renderSubview() {
    switch (settingsSubview.value) {
      case 'system': return <SystemSubmenu />;
      case 'system-overview': return <SystemPage panel="overview" />;
      case 'release-notices': return <SystemPage panel="release-notices" />;
      case 'whats-new': return <SystemPage panel="whats-new" />;
      case 'thread-queue': return <SystemPage panel="thread-queue" />;
      case 'providers': return <ProvidersPage />;
      case 'models': return modelsSection();
      case 'intelligence': return <IntelligencePage />;
      case 'appearance': return appearanceSection();
      case 'memory': return <SystemPage panel="memory" />;
      case 'devices': return <><SettingsGatewaySwitcher />{devicesPage()}</>;
      case 'users': return <GatewayUsersPage />;
      case 'accounts': return accountsSection();
      case 'backup': return <SystemPage panel="backup" />;
      case 'coding-agents': return codingAgentsSection();
      case 'locale': return <LocaleSection />;
      case 'marketplaces': return <MarketplacesSection />;
      case 'access': return <GatewayAccessPage />;
      case 'permissions': return permissionsSection();
      case 'mcp': return <McpServersPage />;
      case 'webhooks': return <WebhooksPage />;
      case 'event-hooks': return <EventHooksPage />;
      case 'keyboard-shortcuts': return <KeyboardShortcutsSection />;
      case 'disk-usage': return <SystemPage panel="disk-usage" />;
      case 'prompt-footprint': return <SystemPage panel="prompt-footprint" />;
      case 'environment-variables': return <SystemPage panel="environment-variables" />;
      case 'debugging': return <SystemPage panel="debugging" />;
      case 'developer': return <SystemPage panel="developer" />;
      case 'communication-surfaces': return <SystemPage panel="communication-surfaces" />;
      case 'updates': return <GatewayUpdatesPage />;
      default: return null;
    }
  }

  return (
    <div class="content-view active settings-panel">
      {renderSubview()}
    </div>
  );
}
