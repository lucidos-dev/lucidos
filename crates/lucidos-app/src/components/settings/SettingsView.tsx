import { useState, useEffect, useCallback } from 'preact/hooks';
import { currentModel, reasoningEffort, preferences, showToast, showConfirm, oauthAccounts, credentials, chatModels, settingsSubview, settingsScrollTarget, SETTINGS_NAV_ITEMS, animationSpeed, speedMultiplier, repositories } from '../../store/store';
import { devices, getDeviceId, loadDevices, updateDeviceName, toggleDevicePush, removeDevice } from '../../store/actions/devices';
import { setImageModel, setBackgroundModel, setTheme, setFontFamily, setCurrentModel, setReasoningEffort, currentTheme, currentFontFamily, currentUiScale, currentImageModel, currentBackgroundModel, currentVertexRegion, setVertexRegion, currentCaptureContext, setCaptureContext, currentCommandGuard, setCommandGuard, currentCommandGuardJudge, setCommandGuardJudge, currentMobileHeaderSticky, setMobileHeaderSticky, type Theme, type FontFamily } from '../../store/actions/preferences';
import { openScaleModal } from '../shared/scaleModalState';
import { formatDateTime } from '../../utils/formatTime';
import { loadOAuthAccounts, disconnectOAuthAccount, grantOAuthScope } from '../../store/actions/oauth';
import { initPushSubscription } from '../../store/actions/push';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { availableReasoningLevels } from '../../store/models';
import { chatModelOptions, loadChatModels } from '../../store/actions/models';
import { ModelsManager } from './ModelsManager';
import { AnthropicProviderSettings } from './AnthropicProviderSettings';
import { OpenAiProviderSettings } from './OpenAiProviderSettings';
import { Dropdown } from '../shared/Dropdown';
import { MemoryInspector } from './MemoryInspector';
import { BackupSection } from './BackupSection';
import { AllowlistEditor } from './AllowlistEditor';
import { getCcAllowedTools, putCcAllowedTools, getAgentAllowedCommands, putAgentAllowedCommands } from '../../api/client';
import { KeyboardShortcutsSection } from './KeyboardShortcutsSection';
import { DiskUsagePage } from './DiskUsagePage';
import { ChevronRightIcon } from '../shared/icons';
import { CredentialItem } from '../credentials/CredentialItem';
import { openAddCredential, loadCredentials } from '../../store/actions/credentials';
import { loadRepositories } from '../../store/actions/chat';
import { API, mutatingFetch, throwIfNotOk } from '../../api/client';
import { DirectoryPicker } from './DirectoryPicker';
import { LoadableError } from '../shared/LoadableError';
import { openSettingsSubview } from '../../store/actions/menu';
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

function parseUserAgent(ua: string | null): string {
  if (!ua) return 'Unknown device';
  const browser = ua.match(/(?:Chrome|Firefox|Safari|Edge|Opera)\/[\d.]+/)?.[0]
    || ua.match(/(?:Chrome|Firefox|Safari|Edge|Opera)/)?.[0]
    || 'Unknown browser';
  const os = ua.includes('iPhone') ? 'iOS'
    : ua.includes('iPad') ? 'iPadOS'
    : ua.includes('Mac') ? 'macOS'
    : ua.includes('Android') ? 'Android'
    : ua.includes('Windows') ? 'Windows'
    : ua.includes('Linux') ? 'Linux'
    : 'Unknown OS';
  return `${browser} on ${os}`;
}

function DeviceRow({ device, editingId, setEditingId }: {
  device: DeviceInfo;
  editingId: string | null;
  setEditingId: (id: string | null) => void;
}) {
  const [editValue, setEditValue] = useState('');
  const currentDeviceId = getDeviceId();
  const isCurrent = device.id === currentDeviceId;
  const displayName = device.name || device.id;
  const editing = editingId === device.id;
  const inputRef = useCallback((el: HTMLInputElement | null) => {
    if (el) { el.focus(); el.select(); }
  }, []);

  function startEditing() {
    setEditValue(device.name || displayName);
    setEditingId(device.id);
  }

  function saveEdit() {
    setEditingId(null);
    const trimmed = editValue.trim();
    const newName = (trimmed && trimmed !== device.id) ? trimmed : null;
    if (newName !== device.name) {
      void updateDeviceName(device.id, newName);
    }
  }

  function cancelEdit() {
    setEditingId(null);
  }

  async function handleTogglePush() {
    const newEnabled = !device.push_enabled;
    if (newEnabled) {
      if (!isCurrent) {
        showToast('Enable push from that device', 'error');
        return;
      }
      const ok = await initPushSubscription();
      if (!ok) return;
    }
    await toggleDevicePush(device.id, newEnabled);
  }

  return (
    <div class={`list-row ${isCurrent ? 'device-current' : ''}`}>
      <div class="list-row-info">
        <div class="title list-row-name">
          {editing ? (
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
        <div class="list-row-details">
          <span>{parseUserAgent(device.user_agent)}</span>
          {' \u00b7 '}
          <span data-tooltip={formatDateTime(new Date(device.last_seen_at))}>
            {formatTimeAgo(new Date(device.last_seen_at))}
          </span>
        </div>
      </div>
      <div class="list-row-actions">
        <span class={`device-push-label${device.push_enabled ? '' : ' push-disabled'}`}>Push</span>
        <label class="toggle-switch">
          <input
            type="checkbox"
            checked={device.push_enabled}
            onChange={handleTogglePush}
          />
          <span class="toggle-slider" />
        </label>
        {isCurrent ? (
          <span class="action-btn" style="visibility: hidden">Remove</span>
        ) : (
          <button
            class="action-btn action-btn-danger"
            onClick={() => void removeDevice(device.id)}
          >Remove</button>
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
    return (
      <div class="list-row-add-card" onClick={() => setAdding(true)}>
        <div class="list-row-add-icon">+</div>
        <div class="list-row-add-label">Add Repository</div>
      </div>
    );
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
      setAdding(false);
      setName('');
      setPath('');
      void loadRepositories();
    } catch (e) {
      showToast(`Failed to add repository: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  }

  return (
    <div class="list-row repo-add-form">
      <div class="list-row-info" style={{ gap: '0.5rem' }}>
        <input
          class="device-name-input"
          type="text"
          placeholder="Name"
          value={name}
          onInput={(e) => setName((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => { if (e.key === 'Enter') void save(); if (e.key === 'Escape') setAdding(false); }}
          autoFocus
        />
        <div class="repo-path-row">
          <input
            class="device-name-input"
            type="text"
            placeholder="Path (e.g. /Users/me/projects/myrepo)"
            value={path}
            onInput={(e) => setPath((e.target as HTMLInputElement).value)}
            onKeyDown={(e) => { if (e.key === 'Enter') void save(); if (e.key === 'Escape') setAdding(false); }}
          />
          <button class="action-btn" onClick={() => setShowPicker(true)}>Browse</button>
        </div>
      </div>
      <div class="list-row-actions">
        <button class="action-btn action-btn-confirm" disabled={saving || !name.trim() || !path.trim()} onClick={save}>
          {saving ? 'Saving...' : 'Save'}
        </button>
        <button class="action-btn" onClick={() => { setAdding(false); setName(''); setPath(''); }}>Cancel</button>
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

function VertexRegionSetting() {
  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="models:vertex-ai">Vertex AI</div>
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
    </div>
  );
}

export function SettingsView() {
  const loadable = devices.value;
  const showLoading = useDelayedLoading(loadable);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [oauthProvider, setOauthProvider] = useState('');
  const [oauthConnecting, setOauthConnecting] = useState(false);
  // Access preferences.value to subscribe to signal updates, then use typed accessors
  preferences.value;
  const uiScale = currentUiScale();
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
      el.classList.remove('settings-search-highlight');
      void el.offsetWidth; // force reflow so the animation re-triggers on repeat clicks
      el.classList.add('settings-search-highlight');
    }
    settingsScrollTarget.value = null;
  }, [settingsScrollTarget.value, settingsSubview.value]);

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
        {credLoadable.data.map((cred) => (
          <CredentialItem key={cred.service_name} credential={cred} />
        ))}
        <div class="list-row-add-card" onClick={openAddCredential}>
          <div class="list-row-add-icon">+</div>
          <div class="list-row-add-label">Add Credential</div>
        </div>
      </div>
    );
  }

  async function handleConnectProvider() {
    const provider = oauthProvider.trim().toLowerCase();
    if (!provider) return;
    setOauthConnecting(true);
    try {
      await grantOAuthScope(provider, 'openid email profile');
      setOauthProvider('');
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
        {oauthLoadable.data.map(account => (
          <div class="list-row" key={account.id}>
            <div class="list-row-info">
              <div class="title">{account.provider}</div>
              <div class="list-row-details">
                {account.email || 'No email'}
                {account.scopes && <> &middot; {formatScopes(account.scopes)}</>}
              </div>
            </div>
            <div class="list-row-actions">
              <button
                class="action-btn"
                onClick={() => void grantOAuthScope(account.provider, account.scopes)}
              >Reconnect</button>
              <button
                class="action-btn action-btn-danger"
                onClick={() => void disconnectOAuthAccount(account.id, account.provider)}
              >Disconnect</button>
            </div>
          </div>
        ))}
        <div class="list-row oauth-connect-row">
          <div class="oauth-quick-providers">
            {['Google', 'Microsoft', 'GitHub'].map(name => (
              <button
                key={name}
                class={`oauth-quick-btn${oauthProvider.toLowerCase() === name.toLowerCase() ? ' active' : ''}`}
                disabled={oauthConnecting}
                onClick={() => setOauthProvider(prev => prev.toLowerCase() === name.toLowerCase() ? '' : name)}
              >{name}</button>
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
        </div>
      </div>
    );
  }

  function accountsSection() {
    return (
      <>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="accounts:credentials">Credentials</div>
          {credentialsSection()}
        </div>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="accounts:oauth">OAuth</div>
          {oauthSection()}
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
    if (loadable.status !== 'loaded') {
      if (!showLoading) return null;
      return (
        <div class="list-rows">
          <div class="loading-spinner" />
        </div>
      );
    }
    if (loadable.data.length === 0) {
      return <div class="list-rows"><div class="empty-state">No devices registered</div></div>;
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
      <div class="list-rows">
        {sorted.map((device) => (
          <DeviceRow key={device.id} device={device} editingId={editingId} setEditingId={setEditingId} />
        ))}
      </div>
    );
  }

  function repositoriesSection() {
    const repoLoadable = repositories.value;

    if (repoLoadable.status === 'failed') {
      return <div class="list-rows"><LoadableError noun="repositories" error={repoLoadable.error} /></div>;
    }
    if (repoLoadable.status !== 'loaded') {
      return null;
    }

    return (
      <div class="settings-section">
        <div class="settings-section-title">Repositories</div>
        <p class="settings-section-desc">Register local git repositories for Claude Code sessions.</p>
        <div class="list-rows">
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
        </div>
      </div>
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
        <div class="settings-section-title" data-search-anchor="command-safety">Command Safety</div>
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

  /** Permissions subview: the Command Safety guard plus the two per-agent
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
    return (
      <>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:chat">Chat &amp; Triggers</div>
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
        </div>
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:image-generation">Image Generation</div>
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
          <div class="settings-section-title" data-search-anchor="models:background-tasks">Background Tasks</div>
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
          <div class="settings-section-title" data-search-anchor="models:debugging">Debugging</div>
          <div class="settings-row" data-search-anchor="models:capture-context">
            <span class="settings-row-label">Capture context per step</span>
            <label class="toggle-switch">
              <input
                type="checkbox"
                checked={currentCaptureContext()}
                onChange={(e) => void setCaptureContext((e.currentTarget as HTMLInputElement).checked)}
              />
              <span class="toggle-slider" />
            </label>
          </div>
        </div>
        <VertexRegionSetting />
        <div class="settings-section">
          <div class="settings-section-title" data-search-anchor="models:providers">Providers</div>
          <AnthropicProviderSettings />
          <OpenAiProviderSettings />
        </div>
        <ModelsManager />
      </>
    );
  }

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
          <div class="settings-row" data-search-anchor="appearance:animation-speed">
            <span class="settings-row-label">Animation speed</span>
            <div class="settings-row-options" style="gap: 0.5rem; align-items: center">
              <input
                type="range"
                min="-10"
                max="10"
                step="1"
                value={animationSpeed.value}
                onInput={(e) => {
                  animationSpeed.value = parseInt((e.target as HTMLInputElement).value);
                }}
                style="width: 7rem"
              />
              <span class="settings-row-label" style="min-width: 2.5rem; text-align: right">{speedMultiplier.value.toFixed(1)}x</span>
            </div>
          </div>
        </div>
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
      </>
    );
  }

  function renderSubview() {
    switch (settingsSubview.value) {
      case 'models': return modelsSection();
      case 'appearance': return appearanceSection();
      case 'memory': return <MemoryInspector />;
      case 'devices': return devicesSection();
      case 'accounts': return accountsSection();
      case 'backup': return <BackupSection />;
      case 'repositories': return repositoriesSection();
      case 'permissions': return permissionsSection();
      case 'keyboard-shortcuts': return <KeyboardShortcutsSection />;
      case 'disk-usage': return <DiskUsagePage />;
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

  return (
    <div class="content-view active settings-panel">
      {SETTINGS_NAV_ITEMS.map(({ key, label }) => (
        <div class="settings-section" key={key}>
          <div class="settings-section-title settings-nav-row" onClick={() => openSettingsSubview(key)}>
            <span>{label}</span>
            <ChevronRightIcon />
          </div>
        </div>
      ))}
    </div>
  );
}
