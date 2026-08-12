import { useState, useEffect, useCallback } from 'preact/hooks';
import { currentModel, reasoningEffort, preferences, showToast, showConfirm, oauthAccounts, credentials, chatModels, settingsSubview, settingsScrollTarget, SETTINGS_NAV_ITEMS, repositories, knownOAuthProviders, oauthConnectPrefill } from '../../store/store';
import { devices, getDeviceId, loadDevices, updateDeviceName, removeDevice } from '../../store/actions/devices';
import { setImageModel, setBackgroundModel, setTheme, setFontFamily, setCurrentModel, setReasoningEffort, currentTheme, currentFontFamily, currentUiScale, currentImageModel, currentBackgroundModel, currentVertexRegion, setVertexRegion, currentCommandGuard, setCommandGuard, currentCommandGuardJudge, setCommandGuardJudge, currentMobileHeaderSticky, setMobileHeaderSticky, currentInAppBrowser, setInAppBrowser, currentExternalLinkTarget, setExternalLinkTarget, externalLinkTargetConfigurable, currentMaxToolCalls, setMaxToolCalls, estimateTurnDuration, MAX_TOOL_CALLS_MIN, MAX_TOOL_CALLS_REPRESENTABLE, currentStyleOverrides, clearStyleOverrides, type ExternalLinkTarget, type Theme, type FontFamily } from '../../store/actions/preferences';
import { openScaleModal } from '../shared/scaleModalState';
import { applyNavFocus } from '../shared/focusMarker';
import { formatDateTime, formatShortDateWithYear } from '../../utils/formatTime';
import {
  loadOAuthAccounts,
  loadKnownOAuthProviders,
  disconnectOAuthAccount,
  grantOAuthScope,
} from '../../store/actions/oauth';
import {
  connectScopes,
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
import { setDevicePushEnabled } from '../../store/actions/push';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { availableReasoningLevels } from '../../store/models';
import { chatModelOptions, loadChatModels } from '../../store/actions/models';
import { ModelsManager } from './ModelsManager';
import { AnthropicProviderSettings } from './AnthropicProviderSettings';
import { OpenAiProviderSettings } from './OpenAiProviderSettings';
import { OpenRouterProviderSettings } from './OpenRouterProviderSettings';
import { LocalProviderSettings } from './LocalProviderSettings';
import { Dropdown } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { ListRowAddCard } from '../shared/ListRowAddCard';
import { AllowlistEditor } from './AllowlistEditor';
import { getCcAllowedTools, putCcAllowedTools, getAgentAllowedCommands, putAgentAllowedCommands } from '../../api/client';
import { KeyboardShortcutsSection } from './KeyboardShortcutsSection';
import { MarketplacesSection } from './MarketplacesSection';
import { MobileAccessPage } from './MobileAccessPage';
import { NetworkAccessPage } from './NetworkAccessPage';
import { LocaleSection } from './LocaleSection';
import { CodingAgentBinariesSection } from './CodingAgentBinariesSection';
import { SystemPage } from './SystemPage';
import { isTauri, describeDeviceUserAgent } from '../../utils/platform';
import { viewportIsMobile } from '../../utils/viewport';
import { ChevronRightIcon } from '../shared/icons';
import { CredentialItem } from '../credentials/CredentialItem';
import { openAddCredential, loadCredentials } from '../../store/actions/credentials';
import { loadRepositories } from '../../store/actions/chat';
import { API, mutatingFetch, throwIfNotOk } from '../../api/client';
import { DirectoryPicker } from './DirectoryPicker';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { openSettingsSubview } from '../../store/actions/menu';
import { focusFirstFocusableWithin } from '../layout/paneFocus';
import { formatTimeAgo } from '../../utils/formatTime';
import type { DeviceInfo } from '../../api/types';
import type { ImageModel } from '../../store/actions/preferences';
import { errorDetail } from '../../utils/errorDetail';

/** Turn scope URLs into short human-readable labels. */
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
      // Extract the last path segment from URLs like https://www.googleapis.com/auth/drive.file
      const key = s.includes('/') ? s.split('/').pop()! : s;
      // Match against known prefixes
      for (const [prefix, label] of Object.entries(map)) {
        if (key.toLowerCase().includes(prefix)) return label;
      }
      return key;
    })
    .filter((v, i, a) => a.indexOf(v) === i) // dedupe
    .join(', ');
}

/** A toggle switch backed by a `Loadable` preference. Until the preference has
 *  loaded it renders a neutral placeholder pill — NOT a definite on/off position
 *  — so the persisted value mounts in its final spot instead of animating across
 *  from the loading default on every page reload (the `.toggle-slider` knob has a
 *  0.2s transition that would otherwise visibly slide off→on). CSS transitions
 *  don't fire on initial mount, so a freshly-mounted checked toggle lands
 *  silently. The placeholder is a `<span>` (not the real `<label>`) so Preact
 *  replaces the whole subtree when `loaded` flips — guaranteeing the fresh mount
 *  rather than an in-place `checked` update that would animate. */
function LoadableToggle(props: {
  loaded: boolean;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  if (!props.loaded) {
    return <span class="toggle-switch toggle-switch-loading" aria-hidden="true" />;
  }
  return (
    <label class={`toggle-switch${props.disabled ? ' toggle-switch-disabled' : ''}`}>
      <input
        type="checkbox"
        checked={props.checked}
        disabled={props.disabled}
        onChange={(e) => props.onChange((e.currentTarget as HTMLInputElement).checked)}
      />
      <span class="toggle-slider" />
    </label>
  );
}

const THEMES: Array<{ value: Theme; label: string }> = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System' },
];

const FONT_OPTIONS: Array<{ value: FontFamily; label: string }> = [
  { value: 'monospace', label: 'Monospace' },
  { value: 'system', label: 'System' },
  { value: 'inter', label: 'Inter' },
  { value: 'jetbrains-mono', label: 'JetBrains Mono' },
  { value: 'ibm-plex-mono', label: 'IBM Plex Mono' },
  { value: 'fira-code', label: 'Fira Code' },
];

const EXTERNAL_LINK_TARGET_OPTIONS: Array<{ value: ExternalLinkTarget; label: string }> = [
  { value: 'safari', label: 'Safari' },
  { value: 'ask', label: 'Ask (share sheet)' },
  { value: 'in-app', label: 'In-app view' },
];

// Presets for the per-turn tool-call cap. The dropdown is `freeText`, so these
// are a starting point rather than the allowed set: any number can be typed,
// and there is no maximum (see `MAX_TOOL_CALLS_MIN`).
const MAX_TOOL_CALLS_OPTIONS = [
  { value: '50', label: '50' },
  { value: '100', label: '100' },
  { value: '250', label: '250' },
  { value: '500', label: '500 (default)' },
  { value: '1000', label: '1000' },
  { value: '2000', label: '2000' },
  { value: '5000', label: '5000' },
];

const IMAGE_MODELS = [
  { value: 'auto', label: 'Auto' },
  { value: 'imagen-4', label: 'Imagen 4' },
  { value: 'gpt-image-1', label: 'GPT Image 1' },
  { value: 'gpt-image-1.5', label: 'GPT Image 1.5' },
  { value: 'gpt-image-2', label: 'GPT Image 2' },
];

// Curated cheap/fast models for auxiliary background work (title generation,
// image description, memory extraction, command judge). Deliberately a small
// list separate from the full chat registry — these run on every turn, so the
// options are the low-cost tiers. GPT-5.4 mini is the OpenAI option — the
// Flash/Haiku-class peer of the others (not the flagship GPT-5.4 Standard);
// routed via the MemoryExtractor's gpt-* prefix when picked.
const BACKGROUND_MODELS = [
  { value: 'gemini-3.5-flash', label: 'Gemini 3.5 Flash' },
  { value: 'gemini-3-flash-preview', label: 'Gemini 3 Flash' },
  { value: 'claude-haiku-4-5', label: 'Haiku 4.5' },
  { value: 'gpt-5.4-mini', label: 'GPT-5.4 mini' },
];

/** Self-skeletonizing device row: rendered with no props inside a
 *  SkeletonProvider (`<DeviceRow />`) it draws itself as a loading placeholder
 *  via the Sk* leaves; with real props it renders normally. The `editing` state
 *  lives in the parent (`editingId`/`setEditingId`) so the skeleton call passes
 *  nothing. Props are optional only to support that call; real call sites pass
 *  them all. */
function DeviceRow({ device, editingId, setEditingId }: {
  device?: DeviceInfo;
  editingId?: string | null;
  setEditingId?: (id: string | null) => void;
}) {
  const sk = useSkeleton();
  const [editValue, setEditValue] = useState('');
  const currentDeviceId = getDeviceId();
  const isCurrent = !sk && device?.id === currentDeviceId;
  const displayName = device?.name || device?.id || '';
  const editing = !sk && device != null && editingId === device.id;
  const inputRef = useCallback((el: HTMLInputElement | null) => {
    if (el) { el.focus(); el.select(); }
  }, []);

  function startEditing() {
    if (!device) return;
    setEditValue(device.name || displayName);
    setEditingId?.(device.id);
  }

  function saveEdit() {
    if (!device) return;
    setEditingId?.(null);
    const trimmed = editValue.trim();
    const newName = (trimmed && trimmed !== device.id) ? trimmed : null;
    if (newName !== device.name) {
      void updateDeviceName(device.id, newName);
    }
  }

  function cancelEdit() {
    setEditingId?.(null);
  }

  // Web push can only be enabled from the target device itself — a browser can
  // only create its own `pushManager.subscribe()`, never another device's. So
  // enabling push for a non-current device is impossible; the toggle is disabled
  // in that state (see below) rather than firing an error on click. Disabling
  // push remotely (and Remove) stays allowed.
  const enableBlocked = !sk && !isCurrent && !device?.push_enabled;

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
                if (e.key === 'Escape') cancelEdit();
              }}
              onBlur={saveEdit}
              ref={inputRef}
            />
          ) : (
            <span class="device-name" onClick={startEditing}>{displayName}</span>
          )}
          {isCurrent && <span class="device-badge">This device</span>}
        </div>
        {/* Two FIELDS, separated by `.list-row-details`' own 0.75rem flex gap.
            No manual middle-dot glue: it would be its own anonymous flex item
            and pick the gap up on both sides (see the oauth row's note below). */}
        <SkText class="list-row-details" as="div" w="14rem">
          <span>{describeDeviceUserAgent(device?.user_agent)}</span>
          {device && (
            <span data-tooltip={formatDateTime(new Date(device.last_seen_at))}>
              {formatTimeAgo(new Date(device.last_seen_at))}
            </span>
          )}
        </SkText>
      </div>
      <div class="list-row-actions">
        {!sk && <span class={`device-push-label${device?.push_enabled ? '' : ' push-disabled'}`}>Push</span>}
        <SkBlock w="2.25rem" h="1.25rem" round>
          <label
            class={`toggle-switch${enableBlocked ? ' toggle-switch-disabled' : ''}`}
            data-tooltip={enableBlocked ? 'Open Lucidos on that device to enable push' : undefined}
          >
            <input
              type="checkbox"
              checked={device?.push_enabled}
              disabled={enableBlocked}
              onChange={() => {
                if (device) void setDevicePushEnabled(device.id, !device.push_enabled);
              }}
            />
            <span class="toggle-slider" />
          </label>
        </SkBlock>
        <SkBlock w="4.5rem" h="2rem" round>
          {isCurrent ? (
            <span class="action-btn" style="visibility: hidden">Remove</span>
          ) : (
            <button
              class="action-btn action-btn-danger"
              onClick={() => { if (device) void removeDevice(device.id); }}
            >Remove</button>
          )}
        </SkBlock>
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

const VERTEX_REGIONS = [
  { value: 'global', label: 'global' },
  { value: 'eu', label: 'eu (multi-region)' },
  { value: 'europe-west1', label: 'europe-west1 (Belgium)' },
  { value: 'europe-west4', label: 'europe-west4 (Netherlands)' },
  { value: 'europe-west9', label: 'europe-west9 (Paris)' },
  { value: 'europe-north1', label: 'europe-north1 (Finland)' },
  { value: 'us-central1', label: 'us-central1 (Iowa)' },
  { value: 'us-east1', label: 'us-east1 (S. Carolina)' },
  { value: 'us-east4', label: 'us-east4 (N. Virginia)' },
  { value: 'us-east5', label: 'us-east5 (Columbus)' },
  { value: 'us-west1', label: 'us-west1 (Oregon)' },
  { value: 'us-west4', label: 'us-west4 (Las Vegas)' },
  { value: 'asia-southeast1', label: 'asia-southeast1 (Singapore)' },
  { value: 'asia-northeast1', label: 'asia-northeast1 (Tokyo)' },
  { value: 'asia-east1', label: 'asia-east1 (Taiwan)' },
];

/** The Vertex provider block inside the "Providers" section. Unlike the other
 *  providers it has no credential to enter — it authenticates via ambient GCP
 *  Application Default Credentials (`gcloud auth application-default login`), so
 *  its only knob is the region. Renders just the rows (the enclosing "Providers"
 *  `settings-section` is owned by `SettingsView`), matching the other provider
 *  components. */
function VertexProviderSettings() {
  return (
    <>
      <div class="settings-row">
        <span class="settings-row-label" data-search-anchor="models:vertex-ai">
          Vertex AI
          <Explainer title="Vertex AI">
            <p>
              Serves the Claude models (Opus / Sonnet / Haiku) on the{' '}
              <strong>vertex</strong> provider via Google Cloud.
            </p>
            <p>
              No key to enter: it uses your <strong>gcloud</strong> Application Default
              Credentials (<code>gcloud auth application-default login</code>). The region
              below is the only setting.
            </p>
          </Explainer>
        </span>
      </div>
      <div class="settings-row" data-search-anchor="models:region">
        <span class="settings-row-label">Region</span>
        <Dropdown
          options={VERTEX_REGIONS}
          value={currentVertexRegion()}
          freeText
          placeholder="e.g. europe-west1"
          onChange={setVertexRegion}
        />
      </div>
    </>
  );
}

/** Hand the connection to the agent, which reads the same *OAuth provider
 *  registry* and can walk the user through the provider's app console: the one
 *  step this page cannot do for them, since the Client ID only exists once an
 *  app is registered there.
 *
 *  Routed through `handleNavigationRequest` rather than poking compose directly,
 *  so it clears the settings overlay, allocates a fresh draft and focuses the
 *  prompt exactly like every other new-chat entry point. */
function askLucidosToConnectAccount(): void {
  handleNavigationRequest({
    target: 'new-chat',
    prompt:
      'Help me connect an account: register the app with the provider, '
      + 'enter the client ID, and sign in.',
  });
}

export function SettingsView() {
  const loadable = devices.value;
  const showLoading = useDelayedLoading(loadable);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [oauthProvider, setOauthProvider] = useState('');
  const [oauthConnecting, setOauthConnecting] = useState(false);
  /** The scopes a deep link said this connection is for, or null for a bare
   *  sign-in. Cleared once used, or when the user picks a different provider:
   *  Backup's upload scopes mean nothing for the provider they typed instead. */
  const [oauthPurpose, setOauthPurpose] = useState<string | null>(null);
  // A failed or absent registry is not an error state on this page: it means no
  // quick buttons and no autofill, and the typed-name path still connects. So
  // it degrades to an empty list rather than a `LoadableError`.
  const registryLoadable = knownOAuthProviders.value;
  const registryProviders =
    registryLoadable.status === 'loaded' ? registryLoadable.data.providers : [];
  // Access preferences.value to subscribe to signal updates, then use typed accessors
  preferences.value;
  const uiScale = currentUiScale();
  const styleOverrideCount = Object.keys(currentStyleOverrides()).length;
  const imageModel = currentImageModel();

  useEffect(() => {
    if (loadable.status === 'not-loaded') {
      void loadDevices();
    }
  }, []);

  useEffect(() => {
    if (oauthAccounts.value.status === 'not-loaded') {
      void loadOAuthAccounts();
    }
  }, []);

  useEffect(() => {
    if (credentials.value.status === 'not-loaded') {
      void loadCredentials();
    }
  }, []);

  useEffect(() => {
    if (chatModels.value.status === 'not-loaded') {
      void loadChatModels();
    }
  }, []);

  useEffect(() => {
    // Mirrors the devices/credentials/oauth effects above. Previously this
    // ran inside `repositoriesSection()`'s render body, which fires a
    // setState during render and trips preact lint.
    if (repositories.value.status === 'not-loaded') {
      void loadRepositories();
    }
  }, [repositories.value.status]);

  // Subscribe to both signals so this fires after the requested subview has rendered.
  useEffect(() => {
    const target = settingsScrollTarget.value;
    if (!target || settingsSubview.value === 'main') return;
    const el = document.querySelector<HTMLElement>(`[data-search-anchor="${target}"]`);
    if (el) {
      el.scrollIntoView({ block: 'center', behavior: 'smooth' });
      // The anchor can sit on a settings row, a row LABEL span, or a section
      // title. Land the focus marker on the enclosing .settings-row when there is
      // one so the sticky highlight washes the whole row (its CSS gives it a uniform
      // gap on all four sides — a bare label span can't take the marker's vertical
      // margin trick); a section-title anchor (no enclosing row) keeps the marker
      // on itself, where it's already padded uniformly.
      const markEl = (el.closest('.settings-row') as HTMLElement | null) ?? el;
      // Shared navigation focus marker: a sticky background highlight that dissolves
      // on the user's next action, never before its hold has elapsed
      // (components/shared/focusMarker.ts). Same look as chat + plugins.
      applyNavFocus(markEl);
      // Land keyboard focus on the targeted row's control (e.g. the Language
      // dropdown) so a Search Everywhere jump lands focus on the setting itself,
      // not the panel's first control. No-op for a section-title anchor with no
      // focusable child; desktop-only (see focusFirstFocusableWithin).
      focusFirstFocusableWithin(markEl);
    }
    settingsScrollTarget.value = null;
  }, [settingsScrollTarget.value, settingsSubview.value]);

  // The *OAuth provider registry* backs both the quick buttons and the Connect
  // form's autofill, so it is fetched once when Accounts is first shown rather
  // than per press.
  useEffect(() => {
    if (settingsSubview.value !== 'accounts') return;
    if (knownOAuthProviders.value.status !== 'not-loaded') return;
    void loadKnownOAuthProviders();
  }, [settingsSubview.value, knownOAuthProviders.value.status]);

  // Arriving from a deep link (Backup's Connect button) with the provider and
  // what the connection is for. Consumed once and cleared, like
  // `settingsScrollTarget`. Waits for the registry so a known provider's field
  // shows its label rather than its bare id.
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
        {/* Keyed by `id`, never `service_name`: a name stopped identifying a row
            when `auth_type` became the discriminator, so an `oauth_client` app
            registration and an API key for the same provider share a name and
            would collide into one key. */}
        {credLoadable.data.map((cred) => (
          <CredentialItem key={cred.id} credential={cred} />
        ))}
        <ListRowAddCard label="Add Credential" onClick={openAddCredential} />
      </div>
    );
  }

  async function handleConnectProvider() {
    if (!oauthProvider.trim()) return;
    // The id, not the text in the field: a quick button puts "Dropbox" there and
    // the credential's service name is `dropbox`, so sending the label would
    // open a second connection under a name differing only in case.
    const provider = providerToSend(registryProviders, oauthProvider);
    if (!provider) return;
    setOauthConnecting(true);
    try {
      // `oauthPurpose` is what a deep link said this connection is FOR. Requesting
      // it here is what makes one consent screen enough when the user arrived
      // from Backup.
      await grantOAuthScope(provider, connectScopes(oauthPurpose));
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
          // formatScopes drops the always-present openid/email/profile scopes,
          // so an account connected with only those (what the Connect button
          // requests) formats to "". Gate on the LABEL, not on account.scopes —
          // an empty span is still a flex item, and .list-row-details' 0.75rem
          // gap would render it as a trailing hole after the email.
          const scopeLabel = account.scopes ? formatScopes(account.scopes) : '';
          // Asked for but not granted: a provider refusing part of a request is
          // a real state, and before the account recorded what it asked for
          // there was nothing to compare, so it looked exactly like an account
          // nobody had asked. Reconnect is the fix, and it is the button beside
          // this line.
          const shortfall = missingScopes(account);
          // Where to go and what to enable there, from the same registry row the
          // Connect form renders. Reconnect on its own grants the same narrow
          // set again whenever the provider's console is the thing that refused
          // it, so naming the shortfall without naming the console leaves the
          // user pressing a button that cannot work.
          const consoleRow = reauthorizationHint(
            registryProviders,
            account.provider,
            shortfall.length > 0,
          );
          return (
          <div class="list-row oauth-account-row" key={account.id}>
            <div class="list-row-info">
              <div class="title list-row-name">{account.provider}</div>
              {/* .list-row-details is a flex row whose 0.75rem gap IS the
                  separator — same as the credential row's url/type pair and the
                  app row's description. Each field is its own element; manual
                  "·" glue would be double-spaced by that gap. */}
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
              {/* The DESIRED set, not the granted one. Re-requesting what the
                  account already holds made the engine's merge compute
                  `granted UNION granted`, so this button could never recover a
                  scope a provider had refused, which is the one thing the
                  engine's own permission errors send the user here to do. */}
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
          {/* One button per known provider, straight from the registry. This was
              a hardcoded `['Google', 'Microsoft', 'GitHub']` array, which is why
              Dropbox had no button despite the engine knowing its endpoints all
              along. Adding a provider to the JSON now adds its button. */}
          <div class="oauth-quick-providers">
            {registryProviders.map(p => (
              <button
                key={p.id}
                class={`oauth-quick-btn${oauthProvider.toLowerCase() === p.label.toLowerCase() ? ' active' : ''}`}
                disabled={oauthConnecting}
                onClick={() => {
                  // Switching provider drops any purpose a deep link supplied:
                  // Backup's upload scopes mean nothing for a different service.
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
          {/* Connecting a provider needs a Client ID out of that provider's own
              app console, with the redirect URI registered byte for byte. The
              form says which console and shows the URI, but the agent can walk
              someone through the console itself, which no static form can. */}
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

  // `accounts-section` full-bleeds the list rows out of .settings-panel's
  // gutter so they sit flush like the Apps and Triggers panels — see
  // styles/settings/accounts.css.
  function accountsSection() {
    return (
      <>
        {/* Both sections carry a one-line explainer. They used to be bare
            "Credentials" and "OAuth" headings, which left a user with no way to
            tell what the difference was, or why connecting Dropbox had produced
            an entry in each (2026-08-05). Credentials is the secret store;
            Connected accounts is the sign-in list. */}
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
            {/* Deliberately does NOT spell out the `oauth:<provider>` service
                name. That name is the storage key, and the row title drops the
                prefix on purpose (see `credentialRowLabel`), so naming it here
                sent the user looking for a string that appears nowhere on the
                screen. The row explains itself instead: provider title, type
                badge, and a note saying which account it belongs to. */}
            <Explainer title="Credentials">
              <p>
                Secrets Lucidos stores for you: API keys, tokens and passwords, and the
                OAuth app registrations behind the accounts above.
              </p>
            </Explainer>
          </div>
          {credentialsSection()}
        </div>
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
    return (
      <div class="list-rows">
        <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf containerClass="list-rows" row={() => <DeviceRow />} />}>
          {loadable.status === 'loaded'
            ? (() => {
                if (loadable.data.length === 0) {
                  return <div class="empty-state">No devices registered</div>;
                }
                // Current device first; then push-enabled; within each group keep the
                // backend's last_seen_at DESC ordering via Array.prototype.sort's ES2019
                // stability guarantee.
                const currentId = getDeviceId();
                const sorted = [...loadable.data].sort((a, b) => {
                  if (a.id === currentId) return -1;
                  if (b.id === currentId) return 1;
                  return Number(b.push_enabled) - Number(a.push_enabled);
                });
                return (
                  <>
                    {sorted.map((device) => (
                      <DeviceRow key={device.id} device={device} editingId={editingId} setEditingId={setEditingId} />
                    ))}
                  </>
                );
              })()
            : null}
        </LoadingFade>
      </div>
    );
  }

  function repositoriesSection() {
    const repoLoadable = repositories.value;

    // The section header renders in EVERY state, because it carries
    // `data-search-anchor="coding-agents:repositories"` and that anchor is a
    // navigation target (Search Everywhere, and the compose picker's "Register
    // a repository" row). `SettingsView`'s scroll effect below does one
    // `querySelector` on the commit where the subview mounts and then clears
    // the target whether or not it matched, so an anchor that waits for a fetch
    // is missed on a cold open and the jump silently lands at the top of the
    // page. Only the LIST is Loadable-gated.
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
                      if (await showConfirm(`Remove "${repo.name}"?`, 'Remove')) {
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

  /** Coding Agents: which `claude` / `codex` binary a coding-agent thread runs,
   *  and which repositories it may run in. Two halves of the same setup that
   *  used to sit in different places (the binaries under System → Overview, the
   *  repositories as their own top-level category). */
  function codingAgentsSection() {
    return (
      <>
        <CodingAgentBinariesSection />
        {repositoriesSection()}
      </>
    );
  }

  /** Access: how you reach this engine from somewhere else. The mobile-access
   *  guide and the engine's network bind are two halves of one question, and
   *  the guide's own "Local network is off" row used to have to deep-link into
   *  System → Network access to finish the job. Now it scrolls.
   *
   *  The two halves each fetch `GET /api/v1/network-config` on mount, so opening
   *  this page costs two requests for one payload. Left as two independent
   *  `Loadable`s deliberately, because they read DISJOINT fields (the guide uses
   *  `gateway_bind` / `detected_tailscale_ip`, the bind editor uses
   *  `engine_bind` / `inherit`) and the call is a cheap local one. That
   *  disjointness is the invariant: give both halves a field in common and one
   *  side goes stale after the other's Save, at which point the fetch has to be
   *  hoisted into a single owner rather than duplicated. */
  function accessSection() {
    return (
      <>
        <MobileAccessPage />
        <NetworkAccessPage />
      </>
    );
  }

  function commandSafetySection() {
    // Parent master (`command_guard`) gates the whole feature; the children
    // (LLM judge on/off + judge model) are independent but only take effect —
    // and are only enabled — while the master is on.
    // `command_guard` / `command_guard_judge` default to a definite value while
    // `preferences` is still loading; gate the toggles on the loaded status so
    // they mount in their persisted position rather than animating across on
    // reload (see `LoadableToggle`).
    const loaded = preferences.value.status === 'loaded';
    const guardOn = currentCommandGuard();
    const judgeOn = currentCommandGuardJudge();
    return (
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="command-safety">Command safety</div>
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
        <div class="settings-row settings-row-child" data-search-anchor="command-safety:judge-model">
          <span class="settings-row-label">Judge model</span>
          <Dropdown
            options={BACKGROUND_MODELS}
            value={currentBackgroundModel('model_command_judge')}
            onChange={(v) => void setBackgroundModel('model_command_judge', v)}
            disabled={!guardOn || !judgeOn}
          />
        </div>
      </div>
    );
  }

  /** Permissions subview: the command safety guard plus the two per-agent
   *  command/tool allowlists, each an editable + deletable list. */
  function permissionsSection() {
    return (
      <>
        {commandSafetySection()}
        <AllowlistEditor
          title="Lucidos Agent permissions"
          anchor="permissions:lucidos"
          noun="command permissions"
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
          title="Claude Code permissions"
          anchor="permissions:claude-code"
          noun="tool permissions"
          placeholder="Bash(npm:*)"
          load={getCcAllowedTools}
          save={putCcAllowedTools}
          description={
            <>
              Patterns passed to Claude Code as <code>--allowedTools</code>.
              Use the <strong>Always allow</strong> buttons on permission prompts to add entries quickly. Changes apply to new Claude Code sessions.
            </>
          }
        />
      </>
    );
  }

  function modelsSection() {
    const maxToolCalls = currentMaxToolCalls();

    /** The dropdown is `freeText`, so this takes whatever the user typed. Only
     *  a whole number is a cap; anything else is rejected with a toast rather
     *  than silently coerced, because a silent coercion of "1oo" to 1 would set
     *  a cap far tighter than the user meant. Integers only, matching the
     *  engine's `parse::<usize>()` (a bare `parseInt` would read "12.5" as 12).
     *  Below the floor is a typo more often than an intent, but the floor is
     *  what the engine would apply anyway, so it saves rather than rejects. */
    function handleMaxToolCallsChange(raw: string) {
      const trimmed = raw.trim();
      if (!/^\d+$/.test(trimmed)) {
        showToast(`"${raw}" is not a whole number of tool calls`, 'error');
        return;
      }
      const parsed = Number(trimmed);
      // Not the policy ceiling this setting deliberately omits: past
      // MAX_SAFE_INTEGER, JS rounds the number, so saving it would store
      // something other than what was typed (see MAX_TOOL_CALLS_REPRESENTABLE).
      if (parsed > MAX_TOOL_CALLS_REPRESENTABLE) {
        showToast(
          `${trimmed} is too large to store exactly (max ${MAX_TOOL_CALLS_REPRESENTABLE.toLocaleString()})`,
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
                The model and reasoning here are the defaults for new chat threads
                and for every trigger.
              </p>
              <p>
                A trigger can pin its own instead, on its edit form in the Triggers
                panel.
              </p>
            </Explainer>
          </div>
          <div class="settings-row">
            <span class="settings-row-label">Model</span>
            <Dropdown
              options={chatModelOptions()}
              value={currentModel.value}
              onChange={(v) => void setCurrentModel(v)}
            />
          </div>
          <div class="settings-row" data-search-anchor="models:reasoning">
            <span class="settings-row-label">Reasoning</span>
            <Dropdown
              options={availableReasoningLevels(currentModel.value)}
              value={reasoningEffort.value}
              onChange={(v) => void setReasoningEffort(v)}
            />
          </div>
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
                  There's no maximum, but the cap is what bounds a runaway turn, and
                  it's roughly how long one can run. Cost grows faster than time,
                  because every step resends the conversation.
                </p>
              </Explainer>
            </span>
            <Dropdown
              options={MAX_TOOL_CALLS_OPTIONS}
              value={String(maxToolCalls)}
              freeText
              placeholder="e.g. 500"
              onChange={handleMaxToolCallsChange}
            />
          </div>
          {/* Stays at rest while the rest of the note moved behind the
              explainer: it is COMPUTED from the value in the field above, and a
              dialog cannot show a figure the user is reading off the control it
              describes. */}
          <div class="settings-row-note">
            {maxToolCalls.toLocaleString()} allows up to about{' '}
            {estimateTurnDuration(maxToolCalls)} of work.
          </div>
        </div>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:image-generation">Image generation</div>
          <div class="settings-row">
            <span class="settings-row-label">Model</span>
            <Dropdown
              options={IMAGE_MODELS}
              value={imageModel}
              onChange={(v) => void setImageModel(v as ImageModel)}
            />
          </div>
        </div>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:background-tasks">Background tasks</div>
          <div class="settings-row" data-search-anchor="models:title-generation">
            <span class="settings-row-label">Title generation</span>
            <Dropdown
              options={BACKGROUND_MODELS}
              value={currentBackgroundModel('model_title')}
              onChange={(v) => void setBackgroundModel('model_title', v)}
            />
          </div>
          <div class="settings-row" data-search-anchor="models:image-description">
            <span class="settings-row-label">Image description</span>
            <Dropdown
              options={BACKGROUND_MODELS}
              value={currentBackgroundModel('model_image_description')}
              onChange={(v) => void setBackgroundModel('model_image_description', v)}
            />
          </div>
          <div class="settings-row" data-search-anchor="models:memory-context">
            <span class="settings-row-label">Memory & context</span>
            <Dropdown
              options={BACKGROUND_MODELS}
              value={currentBackgroundModel('model_memory')}
              onChange={(v) => void setBackgroundModel('model_memory', v)}
            />
          </div>
        </div>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:providers">Providers</div>
          <VertexProviderSettings />
          <AnthropicProviderSettings />
          <OpenAiProviderSettings />
          <OpenRouterProviderSettings />
          <LocalProviderSettings />
        </div>
        <ModelsManager />
      </>
    );
  }

  /** Appearance & Behavior: how the client looks and behaves on this device.
   *  The label gained "& Behavior" when link routing moved in, since where a
   *  link opens is behaviour rather than display. The key stays `appearance`,
   *  the standard name for this category and the head noun of the label, the
   *  same way "Chat & triggers" is anchored `models:chat`. */
  function appearanceSection() {
    const theme = currentTheme();
    const font = currentFontFamily();

    return (
      <>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="appearance:theme">Theme</div>
          <div class="settings-row" data-search-anchor="appearance:mode">
            <span class="settings-row-label">Mode</span>
            <div class="settings-row-options">
              {THEMES.map((t) => (
                <button
                  key={t.value}
                  class={`settings-option ${theme === t.value ? 'active' : ''}`}
                  onClick={() => void setTheme(t.value)}
                >
                  {t.label}
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
              options={FONT_OPTIONS}
              value={font}
              onChange={(v) => void setFontFamily(v as FontFamily)}
            />
          </div>
          <div class="settings-row" data-search-anchor="appearance:ui-scale">
            <span class="settings-row-label">UI scale</span>
            <button class="settings-option" onClick={openScaleModal}>
              {uiScale}%
            </button>
          </div>
        </div>
        {/* The live style remote's undo. A custom property override is applied
            to <html> the moment it is written, so a bad value (a background
            that matches the text, a zero font size) is one slider away, and the
            way out must not depend on the tuned UI being legible. Two routes,
            deliberately: this row, and the `?style-reset` URL parameter, which
            clears the map before first paint and so works even when this row
            cannot be read. The row renders only while overrides exist, so it
            costs nothing to a user who has never opened the remote. */}
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
        {/* The mobile header (and its hide-on-scroll behavior) only exists at the
            ≤768px breakpoint, so this toggle does nothing observable on desktop.
            Gate the whole section on the reactive viewport signal: it appears /
            disappears live as the viewport crosses the breakpoint. The preference
            is global, so it's set from a mobile-width viewport where its effect is
            visible. */}
        {viewportIsMobile.value && (
          <div class="settings-section">
            <div class="settings-section-title" data-search-anchor="appearance:mobile">Mobile</div>
            <div class="settings-row" data-search-anchor="appearance:mobile-header-sticky">
              <span class="settings-row-label">Keep header visible</span>
              <label class="toggle-switch">
                <input
                  type="checkbox"
                  checked={currentMobileHeaderSticky()}
                  onChange={(e) => void setMobileHeaderSticky((e.currentTarget as HTMLInputElement).checked)}
                />
                <span class="toggle-slider" />
              </label>
            </div>
          </div>
        )}
        {notificationsSection()}
        {linksSection()}
      </>
    );
  }

  /** Notifications: the push switch for THIS device, the same one the device's
   *  own row in Settings → Devices carries. Duplicated here on purpose. Push is
   *  per device, and the switch someone reaches for is the one governing the
   *  device in their hand, which is behaviour of this client rather than a fact
   *  about the fleet; Devices stays the place to see and manage the others.
   *  Both rows call `setDevicePushEnabled`, so they cannot disagree. */
  function notificationsSection() {
    const currentId = getDeviceId();
    const current = loadable.status === 'loaded'
      ? loadable.data.find((d) => d.id === currentId)
      : undefined;
    // Loaded, but this device is not in the list: registration failed earlier
    // this page load (`registerCurrentDevice` swallows its own errors as
    // telemetry). Say so, rather than leaving a toggle that can never settle.
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
                {/* The Devices link is why this takes the function form: it
                    navigates, and the dialog has to go with it or it is left
                    floating over the page it just opened. */}
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
      </div>
    );
  }

  /** Links: where a link opens when you tap it. ONE section for one user
   *  question, with a platform-conditional ROW for each answer:
   *
   *   - the external-link target, on an installed iOS PWA (the only client
   *     where the choice has any effect: see `externalLinkTargetConfigurable`);
   *   - the in-app browser, under Tauri (the webview it opens is desktop-only).
   *
   *  Those two used to be separate top-level categories, `Links` and
   *  `Experimental`, each gated so it vanished off its platform. That is how the
   *  external-link setting shipped unreachable on the one platform it applies
   *  to: it sat inside `Experimental`, whose nav entry was `isTauri()`-only. The
   *  gating now lives HERE, on the rows, and the category above it
   *  (Appearance & Behavior) renders Theme + Typography unconditionally, so no
   *  page can come up empty.
   *  Pinned by `__tests__/settings-nav-structure.test.ts`. */
  function linksSection() {
    const showExternalTarget = externalLinkTargetConfigurable();
    const showInAppBrowser = isTauri();
    if (!showExternalTarget && !showInAppBrowser) return null;
    // Gate the toggle on the loaded status so it mounts in its persisted
    // position rather than animating across on reload (see `LoadableToggle`).
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
                {/* The per-link override is iOS's OWN long-press menu, not something
                    we render: suppressing it to draw our own would cost Copy Link,
                    Share and Add to Reading List alongside Open in Safari. Naming it
                    here is the documented way to make it discoverable. */}
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
      case 'system': return <SystemPage />;
      case 'whats-new': return <SystemPage panel="whats-new" />;
      case 'thread-queue': return <SystemPage panel="thread-queue" />;
      case 'models': return modelsSection();
      case 'appearance': return appearanceSection();
      case 'memory': return <SystemPage panel="memory" />;
      case 'devices': return devicesSection();
      case 'accounts': return accountsSection();
      case 'backup': return <SystemPage panel="backup" />;
      case 'coding-agents': return codingAgentsSection();
      case 'locale': return <LocaleSection />;
      case 'marketplaces': return <MarketplacesSection />;
      case 'access': return accessSection();
      case 'permissions': return permissionsSection();
      case 'keyboard-shortcuts': return <KeyboardShortcutsSection />;
      case 'disk-usage': return <SystemPage panel="disk-usage" />;
      case 'environment-variables': return <SystemPage panel="environment-variables" />;
      case 'debugging': return <SystemPage panel="debugging" />;
      default: return null;
    }
  }

  if (settingsSubview.value !== 'main') {
    return (
      <div class="content-view active settings-panel">
        {renderSubview()}
      </div>
    );
  }

  // EVERY nav item is rendered on EVERY platform. There is deliberately no
  // filter here: a category that disappears per-device gives the app a
  // different shape per device, makes "go to Settings → X" false for most
  // users, and is how the iOS external-link setting once became unreachable on
  // the only platform it applies to (it sat inside Experimental, whose row was
  // isTauri()-gated). Platform gating belongs to a row or section INSIDE a
  // category, where an absent control just means one fewer row on a page that
  // still has others. See `linksSection` and the SETTINGS_NAV_ITEMS comment;
  // pinned by `__tests__/settings-nav-structure.test.ts`.
  //
  // Groups are contiguous in SETTINGS_NAV_ITEMS, so a heading is emitted
  // whenever the group changes rather than by pre-bucketing the list.
  return (
    <div class="content-view active settings-panel">
      {SETTINGS_NAV_ITEMS.map(({ key, label, group }, i) => (
        <div class="settings-section settings-nav-item" key={key}>
          {group !== SETTINGS_NAV_ITEMS[i - 1]?.group && (
            <div class="settings-nav-group-title">{group}</div>
          )}
          {/* A real <button>, not a clickable div: these rows are the only way
              into a settings category, so a div here puts every category out of
              keyboard reach (and with it every control inside one). */}
          <button
            type="button"
            class="settings-section-title settings-nav-row"
            onClick={() => openSettingsSubview(key)}
          >
            <span>{label}</span>
            <ChevronRightIcon />
          </button>
        </div>
      ))}
    </div>
  );
}
