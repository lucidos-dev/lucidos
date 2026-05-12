import type { SettingsSubview } from '../../store/store';
import type { SearchResultItem } from '../../api/client';

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
  { id: 'shortcuts', label: 'Keyboard Shortcuts', subview: 'shortcuts', path: 'Settings' },
  { id: 'devices', label: 'Devices', subview: 'devices', path: 'Settings' },
  { id: 'accounts', label: 'Accounts', subview: 'accounts', path: 'Settings' },
  { id: 'repositories', label: 'Repositories', subview: 'repositories', path: 'Settings' },
  { id: 'backup', label: 'Backup', subview: 'backup', path: 'Settings' },
  { id: 'memory', label: 'Memory', subview: 'memory', path: 'Settings' },

  // Models subview
  { id: 'models:chat', label: 'Chat', subview: 'models', path: 'Settings → Models', anchor: 'models:chat' },
  { id: 'models:image-generation', label: 'Image Generation', subview: 'models', path: 'Settings → Models', anchor: 'models:image-generation' },
  { id: 'models:background-tasks', label: 'Background Tasks', subview: 'models', path: 'Settings → Models', anchor: 'models:background-tasks' },
  { id: 'models:vertex-ai', label: 'Vertex AI', subview: 'models', path: 'Settings → Models', anchor: 'models:vertex-ai' },
  { id: 'models:reasoning', label: 'Reasoning', subview: 'models', path: 'Settings → Models → Chat', anchor: 'models:reasoning' },
  { id: 'models:title-generation', label: 'Title generation', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:title-generation' },
  { id: 'models:image-description', label: 'Image description', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:image-description' },
  { id: 'models:memory-context', label: 'Memory & context', subview: 'models', path: 'Settings → Models → Background Tasks', anchor: 'models:memory-context' },
  { id: 'models:region', label: 'Region', subview: 'models', path: 'Settings → Models → Vertex AI', anchor: 'models:region' },

  // Appearance subview
  { id: 'appearance:theme', label: 'Theme', subview: 'appearance', path: 'Settings → Appearance', anchor: 'appearance:theme' },
  { id: 'appearance:typography', label: 'Typography', subview: 'appearance', path: 'Settings → Appearance', anchor: 'appearance:typography' },
  { id: 'appearance:mode', label: 'Mode', subview: 'appearance', path: 'Settings → Appearance → Theme', anchor: 'appearance:mode' },
  { id: 'appearance:font', label: 'Font', subview: 'appearance', path: 'Settings → Appearance → Typography', anchor: 'appearance:font' },
  { id: 'appearance:ui-scale', label: 'UI scale', subview: 'appearance', path: 'Settings → Appearance → Typography', anchor: 'appearance:ui-scale' },
  { id: 'appearance:animation-speed', label: 'Animation speed', subview: 'appearance', path: 'Settings → Appearance → Typography', anchor: 'appearance:animation-speed' },

  // Keyboard Shortcuts subview
  { id: 'shortcuts:navigation', label: 'Navigation', subview: 'shortcuts', path: 'Settings → Keyboard Shortcuts', anchor: 'shortcuts:navigation' },
  { id: 'shortcuts:view', label: 'View', subview: 'shortcuts', path: 'Settings → Keyboard Shortcuts', anchor: 'shortcuts:view' },

  // Backup subview
  { id: 'backup:restore', label: 'Restore from backup', subview: 'backup', path: 'Settings → Backup', anchor: 'backup:restore' },
  { id: 'backup:provider', label: 'Provider', subview: 'backup', path: 'Settings → Backup', anchor: 'backup:provider' },

  // Accounts subview
  { id: 'accounts:credentials', label: 'Credentials', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:credentials' },
  { id: 'accounts:oauth', label: 'OAuth', subview: 'accounts', path: 'Settings → Accounts', anchor: 'accounts:oauth' },
];

/** Filter the index by query (case-insensitive substring on label) and return as SearchResultItems. */
export function getSettingsSearchResults(query: string, limit: number): SearchResultItem[] {
  const q = query.trim().toLowerCase();
  const matches = q
    ? SETTINGS_SEARCH_INDEX.filter(e => e.label.toLowerCase().includes(q))
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
  return SETTINGS_SEARCH_INDEX.find(e => e.id === id);
}
