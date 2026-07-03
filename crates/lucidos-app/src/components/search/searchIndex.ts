import type { SettingsSubview } from '../../store/store';
import type { SearchResultItem } from '../../api/client';
import { SHORTCUT_DEFS, bindingSearchText } from '../../utils/shortcuts';
import { displayBinding, bindingFor } from '../../store/actions/keybindings';
import { isMobile } from '../../utils/viewport';

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
}

/**
 * Searchable Settings entries. Top-level entries (no anchor) just open the subview.
 * Nested entries scroll to a `[data-search-anchor]` element and pulse it.
 *
 * Keep this list in sync with the labels rendered in SettingsView.tsx and BackupSection.tsx.
 */
const SETTINGS_SEARCH_INDEX: SettingsSearchEntry[] = [
  // Top-level subviews
  { id: 'models', label: 'Models', subview: 'models', path: 'Settings' },
  { id: 'appearance', label: 'Appearance', subview: 'appearance', path: 'Settings' },
  { id: 'devices', label: 'Devices', subview: 'devices', path: 'Settings' },
  { id: 'accounts', label: 'Accounts', subview: 'accounts', path: 'Settings' },
  { id: 'repositories', label: 'Repositories', subview: 'repositories', path: 'Settings' },
  { id: 'permissions', label: 'Permissions', subview: 'permissions', path: 'Settings', keywords: 'permissions security command guard safety allowlist claude code lucidos agent bash python tools' },
  { id: 'keyboard-shortcuts', label: 'Keyboard Shortcuts', subview: 'keyboard-shortcuts', path: 'Settings', keywords: 'keybindings hotkeys shortcut' },
  { id: 'system', label: 'System', subview: 'system', path: 'Settings', keywords: 'connection status workspace path api versions build uptime restart refresh update' },

  // System subpanels
  { id: 'backup', label: 'Backup', subview: 'backup', path: 'Settings → System' },
  { id: 'memory', label: 'Memory', subview: 'memory', path: 'Settings → System' },
  { id: 'disk-usage', label: 'Disk Usage', subview: 'disk-usage', path: 'Settings → System', keywords: 'storage space disk usage data' },
  { id: 'environment-variables', label: 'Environment Variables', subview: 'environment-variables', path: 'Settings → System', keywords: 'env var environment variable config' },
  { id: 'debugging', label: 'Debugging', subview: 'debugging', path: 'Settings → System', keywords: 'debug developer diagnostics perf performance instrumentation telemetry lag latency profiling capture context' },

  // System → Debugging rows
  { id: 'debugging:capture-context', label: 'Capture context per step', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:capture-context', keywords: 'capture context step debug llm prompt' },
  { id: 'debugging:perf', label: 'Perf instrumentation', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:perf', keywords: 'perf performance instrumentation telemetry lag latency profiling thread open render linkify' },
  { id: 'debugging:animation-speed', label: 'Animation speed', subview: 'debugging', path: 'Settings → System → Debugging', anchor: 'debugging:animation-speed', keywords: 'animation speed transition duration slow motion multiplier' },

  // System overview
  { id: 'system:connection', label: 'Connection', subview: 'system', path: 'Settings → System', anchor: 'system:connection', keywords: 'status workspace path api url' },
  { id: 'system:versions', label: 'Versions', subview: 'system', path: 'Settings → System', anchor: 'system:versions', keywords: 'lucidos engine client build release uptime' },
  { id: 'system:maintenance', label: 'Maintenance', subview: 'system', path: 'Settings → System', anchor: 'system:maintenance', keywords: 'restart rebuild refresh update client engine' },
  { id: 'system:locale', label: 'Locale', subview: 'system', path: 'Settings → System', anchor: 'system:locale', keywords: 'language timezone region locale' },
  { id: 'system:coding-agents', label: 'Coding agents', subview: 'system', path: 'Settings → System', anchor: 'system:coding-agents', keywords: 'coding agent claude codex binary path cli override auto-detect' },
  { id: 'system:language', label: 'Language', subview: 'system', path: 'Settings → System → Locale', anchor: 'system:language', keywords: 'language locale respond reply' },
  { id: 'system:timezone', label: 'Timezone', subview: 'system', path: 'Settings → System → Locale', anchor: 'system:timezone', keywords: 'timezone time zone iana triggers schedule' },

  // Models subview
  { id: 'models:chat', label: 'Chat', subview: 'models', path: 'Settings → Models', anchor: 'models:chat' },
  { id: 'models:image-generation', label: 'Image Generation', subview: 'models', path: 'Settings → Models', anchor: 'models:image-generation' },
  { id: 'models:background-tasks', label: 'Background Tasks', subview: 'models', path: 'Settings → Models', anchor: 'models:background-tasks' },
  { id: 'models:vertex-ai', label: 'Vertex AI', subview: 'models', path: 'Settings → Models → Providers', anchor: 'models:vertex-ai', keywords: 'vertex gcloud gcp google adc region' },
  { id: 'models:providers', label: 'Providers', subview: 'models', path: 'Settings → Models', anchor: 'models:providers', keywords: 'providers vertex anthropic openai openrouter local gcloud gcp google api key direct credential gpt claude' },
  { id: 'models:reasoning', label: 'Reasoning', subview: 'models', path: 'Settings → Models → Chat', anchor: 'models:reasoning' },
  { id: 'models:title-generation', label: 'Title generation', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:title-generation' },
  { id: 'models:image-description', label: 'Image description', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:image-description' },
  { id: 'models:memory-context', label: 'Memory & context', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:memory-context' },
  { id: 'models:region', label: 'Region', subview: 'models', path: 'Settings → Models → Providers → Vertex AI', anchor: 'models:region' },

  // Appearance subview
  { id: 'appearance:theme', label: 'Theme', subview: 'appearance', path: 'Settings → Appearance', anchor: 'appearance:theme' },
  { id: 'appearance:typography', label: 'Typography', subview: 'appearance', path: 'Settings → Appearance', anchor: 'appearance:typography' },
  { id: 'appearance:mode', label: 'Mode', subview: 'appearance', path: 'Settings → Appearance → Theme', anchor: 'appearance:mode' },
  { id: 'appearance:font', label: 'Font', subview: 'appearance', path: 'Settings → Appearance → Typography', anchor: 'appearance:font' },
  { id: 'appearance:ui-scale', label: 'UI scale', subview: 'appearance', path: 'Settings → Appearance → Typography', anchor: 'appearance:ui-scale' },
  { id: 'appearance:mobile', label: 'Mobile', subview: 'appearance', path: 'Settings → Appearance', anchor: 'appearance:mobile', mobileOnly: true },
  { id: 'appearance:mobile-header-sticky', label: 'Keep header visible', subview: 'appearance', path: 'Settings → Appearance → Mobile', anchor: 'appearance:mobile-header-sticky', mobileOnly: true },

  // Backup subview (restore moved to the workspace picker — no in-app entry)
  { id: 'backup:provider', label: 'Provider', subview: 'backup', path: 'Settings → System → Backup', anchor: 'backup:provider' },

  // Accounts subview
  { id: 'accounts:credentials', label: 'Credentials', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:credentials' },
  { id: 'accounts:oauth', label: 'OAuth', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:oauth' },

  // Permissions subview (Command Safety + the two allowlist editors)
  { id: 'command-safety', label: 'Command Safety', subview: 'permissions', path: 'Settings → Permissions', anchor: 'command-safety', keywords: 'command guard safety bash python shell judge' },
  { id: 'command-safety:guard', label: 'Command guard', subview: 'permissions', path: 'Settings → Permissions → Command Safety', anchor: 'command-safety:guard', keywords: 'command guard safety bash python shell' },
  { id: 'command-safety:judge', label: 'LLM judge', subview: 'permissions', path: 'Settings → Permissions → Command Safety', anchor: 'command-safety:judge', keywords: 'command guard llm judge' },
  { id: 'command-safety:judge-model', label: 'Judge model', subview: 'permissions', path: 'Settings → Permissions → Command Safety', anchor: 'command-safety:judge-model', keywords: 'command guard judge model haiku' },
  { id: 'permissions:lucidos', label: 'Lucidos Agent permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:lucidos', keywords: 'lucidos agent command allowlist bash python always allow auto allow' },
  { id: 'permissions:claude-code', label: 'Claude Code permissions', subview: 'permissions', path: 'Settings → Permissions', anchor: 'permissions:claude-code', keywords: 'claude code coding agent tool permissions allowed tools allowlist' },
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
  const visible = (e: SettingsSearchEntry) => !e.mobileOnly || isMobile();
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
