import { enginePackaged, type SettingsSubview } from '../../store/store';
import type { SearchResultItem } from '../../api/client';
import { SHORTCUT_DEFS, bindingSearchText } from '../../utils/shortcuts';
import { displayBinding, bindingFor } from '../../store/actions/keybindings';
import { isMobile } from '../../utils/viewport';
import { isTauri } from '../../utils/platform';
import { WORKSPACE_ID } from '../../utils/basePath';
// The one definition of "this client can actually act on the external-link
// target", shared with the Settings row and its nav entry so search can never
// offer a result that lands on nothing.
import { externalLinkTargetConfigurable } from '../../store/actions/preferences';

type Subview = Exclude<SettingsSubview, 'main'>;

interface SettingsSearchEntry {
  /** Unique result id; also used as the recents key. */
  id: string;
  /** Label as it appears in the UI — single source of truth (no Rust ↔ UI drift). */
  label: string;
  /** Subview to switch to when selected. */
  subview: Subview;
  /** Breadcrumb path shown as the result subtitle. */
  path: string;
  /** Optional `data-search-anchor` value to scroll/highlight after navigation. */
  anchor?: string;
  /** Extra free-text matched in addition to the label (e.g. key-combo aliases
   *  like "ctrl k" for a shortcut). Not shown in the UI. */
  keywords?: string;
  /** Only surfaced in search results on a mobile-width viewport — the matching
   *  settings row is itself hidden on desktop (e.g. the Mobile section), so a
   *  desktop result would navigate to a row that doesn't render. */
  mobileOnly?: boolean;
  /** Only surfaced on a PACKAGED install (the `packaged` flag from /health), for
   *  the same reason as `mobileOnly`: the matching row renders only there (e.g.
   *  Debugging's Restart engine, whose dev counterpart is Overview's
   *  Rebuild & Restart), so a dev result would land on nothing. */
  packagedOnly?: boolean;
  /** Only surfaced on an INSTALLED iOS PWA, for the same reason as the two
   *  above. Strictly narrower than `mobileOnly`: a narrow desktop window and
   *  mobile Chrome both pass `isMobile()` but are not standalone iOS, and the
   *  row behind this flag (Appearance & Behavior → Links → Open links in)
   *  renders only there. */
  iosPwaOnly?: boolean;
  /** Only surfaced under Tauri, for the same reason again: the in-app browser
   *  opens a desktop webview, so its toggle renders nowhere else. */
  tauriOnly?: boolean;
  /** Only surfaced on a page the workspace gateway served, once more for the
   *  same reason: Paired devices reads the gateway's own auth surface and
   *  renders nothing when there is no gateway to ask. A page on a direct
   *  engine port resolves that path against the engine and gets a 404. */
  gatewayOnly?: boolean;
}

/**
 * Searchable Settings entries. Top-level entries (no anchor) just open the subview.
 * Nested entries scroll to a `[data-search-anchor]` element and pulse it.
 *
 * Keep this list in sync with the labels rendered in SettingsView.tsx and BackupSection.tsx.
 */
const SETTINGS_SEARCH_INDEX: SettingsSearchEntry[] = [
  // Top-level subviews (one per SETTINGS_NAV_ITEMS entry)
  { id: 'models', label: 'Models', subview: 'models', path: 'Settings' },
  { id: 'permissions', label: 'Permissions', subview: 'permissions', path: 'Settings', keywords: 'permissions security command guard safety allowlist claude code lucidos agent bash python tools' },
  { id: 'mcp', label: 'MCP Servers', subview: 'mcp', path: 'Settings', keywords: 'mcp model context protocol server tools context cost tokens start stop remove disable allowlist' },
  { id: 'coding-agents', label: 'Coding Agents', subview: 'coding-agents', path: 'Settings', keywords: 'coding agent claude code codex binary path cli repository repositories git worktree' },
  { id: 'accounts', label: 'Accounts', subview: 'accounts', path: 'Settings' },
  { id: 'locale', label: 'Locale', subview: 'locale', path: 'Settings', keywords: 'language timezone region locale time zone' },
  { id: 'marketplaces', label: 'Marketplaces', subview: 'marketplaces', path: 'Settings', keywords: 'marketplace plugin catalog install source registry' },
  { id: 'access', label: 'Access', subview: 'access', path: 'Settings', keywords: 'mobile access remote phone tailscale tailnet connect url network bind lan pairing code add device' },
  { id: 'webhooks', label: 'Webhooks', subview: 'webhooks', path: 'Settings', keywords: 'webhook inbound hook endpoint github stripe slack signature hmac token funnel event' },
  { id: 'devices', label: 'Devices', subview: 'devices', path: 'Settings', keywords: 'device phone laptop push notifications rename remove last seen' },
  // Pairing lives on the same row as push now, so Revoke is found on the
  // Devices page rather than under Access. Still gateway-gated: with none, no
  // row carries a Revoke button and the word would land on nothing.
  { id: 'devices:paired', label: 'Paired devices', subview: 'devices', path: 'Settings → Devices', anchor: 'devices:list', keywords: 'paired pairing device revoke unpair sign out cut off network access', gatewayOnly: true },
  { id: 'system', label: 'System', subview: 'system', path: 'Settings', keywords: 'connection status workspace path api versions build uptime restart refresh update' },
  { id: 'appearance', label: 'Appearance & Behavior', subview: 'appearance', path: 'Settings', keywords: 'appearance interface behavior theme font scale links browser' },
  { id: 'keyboard-shortcuts', label: 'Keyboard Shortcuts', subview: 'keyboard-shortcuts', path: 'Settings', keywords: 'keybindings hotkeys shortcut' },

  // System subpanels
  { id: 'release-notices', label: 'Release Notices', subview: 'release-notices', path: 'Settings → System', anchor: 'release-notices:list', keywords: 'release notice notices after upgrade to do action needed workspace audit drift got it answered' },
  { id: 'whats-new', label: "What's New", subview: 'whats-new', path: 'Settings → System', keywords: 'changelog release notes version history whats new updates changes released' },
  { id: 'backup', label: 'Backup', subview: 'backup', path: 'Settings → System' },
  { id: 'memory', label: 'Memory', subview: 'memory', path: 'Settings → System' },
  { id: 'disk-usage', label: 'Disk Usage', subview: 'disk-usage', path: 'Settings → System', keywords: 'storage space disk usage data' },
  { id: 'environment-variables', label: 'Environment Variables', subview: 'environment-variables', path: 'Settings → System', keywords: 'env var environment variable config' },
  { id: 'debugging', label: 'Debugging', subview: 'debugging', path: 'Settings → System', keywords: 'debug developer diagnostics perf performance instrumentation telemetry lag latency profiling capture context' },

  // System → Debugging rows
  { id: 'debugging:capture-context', label: 'Capture context per step', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:capture-context', keywords: 'capture context step debug llm prompt' },
  { id: 'debugging:perf', label: 'Perf instrumentation', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:perf', keywords: 'perf performance instrumentation telemetry lag latency profiling thread open render linkify' },
  { id: 'debugging:animation-speed', label: 'Animation speed', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:animation-speed', keywords: 'animation speed transition duration slow motion multiplier' },
  { id: 'debugging:restart-engine', label: 'Restart engine', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:restart-engine', keywords: 'restart engine service launchd relaunch reboot recovery unresponsive stuck', packagedOnly: true },

  // System overview
  { id: 'system:connection', label: 'Connection', subview: 'system', path: 'Settings → System', anchor: 'system:connection', keywords: 'status workspace path api url' },
  { id: 'system:versions', label: 'Versions', subview: 'system', path: 'Settings → System', anchor: 'system:versions', keywords: 'lucidos engine client build release uptime' },
  { id: 'system:maintenance', label: 'Maintenance', subview: 'system', path: 'Settings → System', anchor: 'system:maintenance', keywords: 'restart rebuild refresh update client engine' },

  // Locale subview
  { id: 'locale:language', label: 'Language', subview: 'locale', path: 'Settings → Locale', anchor: 'locale:language', keywords: 'language locale respond reply' },
  { id: 'locale:timezone', label: 'Timezone', subview: 'locale', path: 'Settings → Locale', anchor: 'locale:timezone', keywords: 'timezone time zone iana triggers schedule' },

  // Coding Agents subview
  { id: 'coding-agents:binaries', label: 'Binaries', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:binaries', keywords: 'coding agent claude codex binary path cli override auto-detect' },
  { id: 'coding-agents:permissions', label: 'Permissions', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:permissions', keywords: 'permission mode claude code auto accept edits classifier approve prompt card ask' },
  { id: 'coding-agents:repositories', label: 'Repositories', subview: 'coding-agents', path: 'Settings → Coding Agents', anchor: 'coding-agents:repositories', keywords: 'repository repositories git local clone register external repo' },

  // Access subview
  // Ungated on purpose: the section renders in any browser now, deriving its
  // tailnet rows from two plain-HTTP reads. Gating it hid the address the
  // browser user came looking for, from behind the section showing it.
  { id: 'access:urls', label: 'Connect URLs', subview: 'access', path: 'Settings → Access', anchor: 'access:urls', keywords: 'connect url localhost lan tailnet magicdns address phone open elsewhere' },
  { id: 'access:add-device', label: 'Add a device', subview: 'access', path: 'Settings → Access', anchor: 'access:add-device', keywords: 'pair pairing code qr scan phone new device enrol add' },
  { id: 'access:tailscale', label: 'Tailscale', subview: 'access', path: 'Settings → Access', anchor: 'access:tailscale', keywords: 'tailscale tailnet vpn magicdns serve https sign in' },
  { id: 'access:network', label: 'Network access', subview: 'access', path: 'Settings → Access', anchor: 'access:network', keywords: 'network bind loopback lan address listen expose engine' },

  // Models subview
  { id: 'models:chat', label: 'Chat & triggers', subview: 'models', path: 'Settings → Models', anchor: 'models:chat' },
  { id: 'models:image-generation', label: 'Image generation', subview: 'models', path: 'Settings → Models', anchor: 'models:image-generation' },
  { id: 'models:background-tasks', label: 'Background tasks', subview: 'models', path: 'Settings → Models', anchor: 'models:background-tasks' },
  { id: 'models:vertex-ai', label: 'Vertex AI', subview: 'models', path: 'Settings → Models → Providers', anchor: 'models:vertex-ai', keywords: 'vertex gcloud gcp google adc region' },
  { id: 'models:providers', label: 'Providers', subview: 'models', path: 'Settings → Models', anchor: 'models:providers', keywords: 'providers vertex anthropic openai openrouter xai grok opencode free keyless local gcloud gcp google api key direct credential gpt claude' },
  // Its own row, not just a keyword on the section above. It is the one
  // provider a user with no key can turn on. So "free" lands on the switch,
  // rather than on the top of a page they then have to scan.
  { id: 'models:opencode-free', label: 'OpenCode Free (keyless)', subview: 'models', path: 'Settings → Models → Providers', anchor: 'models:opencode-free', keywords: 'opencode free keyless no key no account zen relay anonymous trial try' },
  { id: 'models:chat-model', label: 'Model', subview: 'models', path: 'Settings → Models → Chat & triggers', anchor: 'models:chat-model', keywords: 'model reasoning effort thinking tier opus sonnet haiku gpt' },
  { id: 'models:max-tool-calls', label: 'Max tool calls', subview: 'models', path: 'Settings → Models → Chat & triggers', anchor: 'models:max-tool-calls', keywords: 'max tool calls cap limit turn runaway budget' },
  { id: 'models:title-generation', label: 'Title generation', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:title-generation' },
  { id: 'models:image-description', label: 'Image description', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:image-description' },
  { id: 'models:memory-extraction', label: 'Memory extraction', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:memory-extraction' },
  { id: 'models:conversation-summary', label: 'Conversation summary', subview: 'models', path: 'Settings → Models → Background tasks', anchor: 'models:conversation-summary' },
  // Lands on the Vertex header, not on the Region row itself. That row sits
  // inside the provider's block, which renders only while Vertex is switched
  // on. An anchor pointing at it scrolls to nothing whenever it is off. The
  // header is always there, and carries the switch that brings the row back.
  { id: 'models:region', label: 'Region', subview: 'models', path: 'Settings → Models → Providers → Vertex AI', anchor: 'models:vertex-ai' },

  // Appearance & Behavior subview (Links absorbed the retired Links and
  // Experimental categories, so its two rows keep their own platform flags)
  { id: 'appearance:theme', label: 'Theme', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:theme' },
  { id: 'appearance:typography', label: 'Typography', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:typography' },
  { id: 'appearance:mode', label: 'Mode', subview: 'appearance', path: 'Settings → Appearance & Behavior → Theme', anchor: 'appearance:mode' },
  { id: 'appearance:font', label: 'Font', subview: 'appearance', path: 'Settings → Appearance & Behavior → Typography', anchor: 'appearance:font' },
  { id: 'appearance:ui-scale', label: 'UI scale', subview: 'appearance', path: 'Settings → Appearance & Behavior → Typography', anchor: 'appearance:ui-scale' },
  { id: 'appearance:mobile', label: 'Mobile', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:mobile', mobileOnly: true },
  { id: 'appearance:mobile-header-sticky', label: 'Keep header visible', subview: 'appearance', path: 'Settings → Appearance & Behavior → Mobile', anchor: 'appearance:mobile-header-sticky', mobileOnly: true },
  // The current device's push switch, the same one its row in Devices carries.
  // Both entries are kept: someone hunting "notifications" means the device they
  // are holding, someone hunting "devices" means the fleet.
  { id: 'appearance:notifications', label: 'Notifications', subview: 'appearance', path: 'Settings → Appearance & Behavior', anchor: 'appearance:notifications', keywords: 'notifications push alerts banners this device' },
  { id: 'appearance:push-notifications', label: 'Push notifications', subview: 'appearance', path: 'Settings → Appearance & Behavior → Notifications', anchor: 'appearance:push-notifications', keywords: 'push notifications enable disable this device alerts banners buzz' },
  // No entry for the Links SECTION itself: it renders only when one of the two
  // rows below does, and `visible` ANDs its flags, so a section entry could not
  // express "iOS PWA OR Tauri" without a one-off predicate. The rows carry the
  // gates, and the `appearance` top-level entry keeps `links` in its keywords so
  // a search for it still lands somewhere true.
  { id: 'appearance:external-link-target', label: 'Open links in', subview: 'appearance', path: 'Settings → Appearance & Behavior → Links', anchor: 'appearance:external-link-target', keywords: 'external links safari ask share sheet in-app browser open link default browser', iosPwaOnly: true },
  { id: 'appearance:in-app-browser', label: 'Open links in the in-app browser', subview: 'appearance', path: 'Settings → Appearance & Behavior → Links', anchor: 'appearance:in-app-browser', keywords: 'in-app browser pane experimental drawer external link', tauriOnly: true },

  // Backup subview (restore moved to the workspace picker — no in-app entry)
  { id: 'backup:provider', label: 'Provider', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:provider' },

  // Accounts subview
  { id: 'accounts:credentials', label: 'Credentials', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:credentials', keywords: 'api key token password secret oauth client app registration' },
  // Renamed from "OAuth", which named the protocol rather than the thing. The
  // old word stays searchable via keywords so nobody loses the entry.
  { id: 'accounts:connected', label: 'Connected accounts', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:connected', keywords: 'oauth connect sign in google microsoft github dropbox account authorize reconnect disconnect' },

  // Permissions subview (Command safety + the two allowlist editors)
  { id: 'command-safety', label: 'Command safety', subview: 'permissions', path: 'Settings → Permissions', anchor: 'command-safety', keywords: 'command guard safety bash python shell judge' },
  { id: 'command-safety:guard', label: 'Command guard', subview: 'permissions', path: 'Settings → Permissions → Command safety', anchor: 'command-safety:guard', keywords: 'command guard safety bash python shell' },
  { id: 'command-safety:judge', label: 'LLM judge', subview: 'permissions', path: 'Settings → Permissions → Command safety', anchor: 'command-safety:judge', keywords: 'command guard llm judge' },
  { id: 'command-safety:judge-model', label: 'Judge model', subview: 'permissions', path: 'Settings → Permissions → Command safety', anchor: 'command-safety:judge-model', keywords: 'command guard judge model haiku' },
  { id: 'permissions:lucidos', label: 'Lucidos Agent permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:lucidos', keywords: 'lucidos agent command allowlist bash python always allow auto allow' },
  { id: 'permissions:claude-code', label: 'Claude Code permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:claude-code', keywords: 'claude code coding agent tool permissions allowed tools allowlist' },
  { id: 'permissions:mcp', label: 'MCP tool permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:mcp', keywords: 'mcp model context protocol server tool permissions allowlist always allow' },

  // MCP Servers subview
  { id: 'mcp:cost', label: 'Context cost', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:cost', keywords: 'mcp context cost tokens window per request tool definitions expensive' },
  { id: 'mcp:servers', label: 'Servers', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:servers', keywords: 'mcp server start stop remove running auto approve disable tool unusable id dispatch' },
  { id: 'mcp:allowed-tools', label: 'MCP tool permissions', subview: 'mcp', path: 'Settings → MCP Servers', anchor: 'mcp:allowed-tools', keywords: 'mcp allowed tools allowlist always allow permission pattern' },
];

/** Per-shortcut search entries, synthesized from the registry so they reflect
 *  the user's CURRENT (possibly-customized) binding. Each carries key-combo
 *  aliases ("ctrl k", "ctrl+k", "cmd k", …) as keywords so typing a combo finds
 *  it; selecting one opens the Keyboard Shortcuts cheat sheet. */
function shortcutSearchEntries(): SettingsSearchEntry[] {
  return SHORTCUT_DEFS.map((def) => ({
    id: `shortcut:${def.id}`,
    label: `${def.label} (${displayBinding(def.id)})`,
    subview: 'keyboard-shortcuts' as Subview,
    path: 'Settings → Keyboard Shortcuts',
    keywords: `${def.label} ${bindingSearchText(bindingFor(def.id))} keyboard shortcut`,
  }));
}

function allSettingsEntries(): SettingsSearchEntry[] {
  return [...SETTINGS_SEARCH_INDEX, ...shortcutSearchEntries()];
}

/** Filter the index by query (case-insensitive substring over label + keywords)
 *  and return as SearchResultItems. An empty query lists the static settings
 *  index only (not every shortcut). */
export function getSettingsSearchResults(query: string, limit: number): SearchResultItem[] {
  const q = query.trim().toLowerCase();
  // Mobile-only rows are hidden in Settings on desktop, so don't surface them as
  // search results there — selecting one would land on a row that doesn't render.
  // Packaged-only rows are gated the same way, against the /health `packaged` flag.
  const visible = (e: SettingsSearchEntry) =>
    (!e.mobileOnly || isMobile())
    && (!e.packagedOnly || enginePackaged.value)
    && (!e.iosPwaOnly || externalLinkTargetConfigurable())
    && (!e.tauriOnly || isTauri())
    && (!e.gatewayOnly || WORKSPACE_ID !== null);
  const matches = q
    ? allSettingsEntries().filter(e => visible(e) && `${e.label} ${e.keywords ?? ''}`.toLowerCase().includes(q))
    : SETTINGS_SEARCH_INDEX.filter(visible);
  return matches.slice(0, limit).map(e => ({
    id: e.id,
    title: e.label,
    subtitle: e.path,
    category: 'settings',
    score: 1.0,
  }));
}

export function findSettingsEntry(id: string): SettingsSearchEntry | undefined {
  return allSettingsEntries().find(e => e.id === id);
}

/** Ids of the STATIC index (not the synthesized per-shortcut entries, which are
 *  uniform and anchor-less). Exists so the settings-nav guard can walk every
 *  entry and check its subview is live and its anchor is rendered, without this
 *  module exporting the whole array. */
export function settingsSearchEntryIds(): string[] {
  return SETTINGS_SEARCH_INDEX.map(e => e.id);
}
