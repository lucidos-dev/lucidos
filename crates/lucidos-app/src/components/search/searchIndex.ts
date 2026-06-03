import type { SettingsSubview } from '../../store/store';
import type { SearchResultItem } from '../../api/client';
import { SHORTCUT_DEFS, bindingSearchText } from '../../utils/shortcuts';
import { displayBinding, bindingFor } from '../../store/actions/keybindings';

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
  { id: 'backup', label: 'Backup', subview: 'backup', path: 'Settings' },
  { id: 'memory', label: 'Memory', subview: 'memory', path: 'Settings' },
  { id: 'keyboard-shortcuts', label: 'Keyboard Shortcuts', subview: 'keyboard-shortcuts', path: 'Settings', keywords: 'keybindings hotkeys shortcut' },

  // Models subview
  { id: 'models:chat', label: 'Chat', subview: 'models', path: 'Settings → Models', anchor: 'models:chat' },
  { id: 'models:image-generation', label: 'Image Generation', subview: 'models', path: 'Settings → Models', anchor: 'models:image-generation' },
  { id: 'models:background-tasks', label: 'Background Tasks', subview: 'models', path: 'Settings → Models', anchor: 'models:background-tasks' },
  { id: 'models:vertex-ai', label: 'Vertex AI', subview: 'models', path: 'Settings → Models', anchor: 'models:vertex-ai' },
  { id: 'models:reasoning', label: 'Reasoning', subview: 'models', path: 'Settings → Models → Chat', anchor: 'models:reasoning' },
  { id: 'models:title-generation', label: 'Title generation', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:title-generation' },
  { id: 'models:image-description', label: 'Image description', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:image-description' },
  { id: 'models:memory-context', label: 'Memory & context', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:memory-context' },
  { id: 'models:debugging', label: 'Debugging', subview: 'models', path: 'Settings → Models', anchor: 'models:debugging' },
  { id: 'models:capture-context', label: 'Capture context per step', subview: 'models', path: 'Settings → Models → Debugging', anchor: 'models:capture-context' },
  { id: 'models:region', label: 'Region', subview: 'models', path: 'Settings → Models → Vertex AI', anchor: 'models:region' },

  // Appearance subview
  { id: 'appearance:theme', label: 'Theme', subview: 'appearance', path: 'Settings → Appearance', anchor: 'appearance:theme' },
  { id: 'appearance:typography', label: 'Typography', subview: 'appearance', path: 'Settings → Appearance', anchor: 'appearance:typography' },
  { id: 'appearance:mode', label: 'Mode', subview: 'appearance', path: 'Settings → Appearance → Theme', anchor: 'appearance:mode' },
  { id: 'appearance:font', label: 'Font', subview: 'appearance', path: 'Settings → Appearance → Typography', anchor: 'appearance:font' },
  { id: 'appearance:ui-scale', label: 'UI scale', subview: 'appearance', path: 'Settings → Appearance → Typography', anchor: 'appearance:ui-scale' },
  { id: 'appearance:animation-speed', label: 'Animation speed', subview: 'appearance', path: 'Settings → Appearance → Typography', anchor: 'appearance:animation-speed' },
  { id: 'appearance:mobile', label: 'Mobile', subview: 'appearance', path: 'Settings → Appearance', anchor: 'appearance:mobile' },
  { id: 'appearance:mobile-header-sticky', label: 'Keep header visible', subview: 'appearance', path: 'Settings → Appearance → Mobile', anchor: 'appearance:mobile-header-sticky' },

  // Backup subview
  { id: 'backup:restore', label: 'Restore from backup', subview: 'backup', path: 'Settings → Backup', anchor: 'backup:restore' },
  { id: 'backup:provider', label: 'Provider', subview: 'backup', path: 'Settings → Backup', anchor: 'backup:provider' },

  // Accounts subview
  { id: 'accounts:credentials', label: 'Credentials', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:credentials' },
  { id: 'accounts:oauth', label: 'OAuth', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:oauth' },
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
  const matches = q
    ? allSettingsEntries().filter(e => `${e.label} ${e.keywords ?? ''}`.toLowerCase().includes(q))
    : SETTINGS_SEARCH_INDEX;
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
