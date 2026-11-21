import { useEffect } from 'preact/hooks';
import { SETTINGS_NAV_ITEMS } from '../../store/store';
import { warmSettingsReads } from '../../store/actions/settingsReads';
import { systemAttentionBadge } from '../../store/systemAttentionBadge';
import { openSettingsSubview } from '../../store/actions/menu';
import { SettingsNavRow } from './SettingsNavRow';

/**
 * The Settings home: one row per category.
 *
 * Eager, unlike `SettingsView`, which is a code-split chunk. A lazy component
 * renders nothing until its chunk lands, and every rebuild renames the chunks.
 * Everything here is already in the main bundle, so the home list never waits
 * on the network.
 *
 * EVERY nav item is rendered on EVERY platform. A category that disappears
 * per device gives the app a different shape per device, and makes "go to
 * Settings → X" false for most users. Platform gating belongs to a row inside
 * a category. Pinned by `__tests__/settings-nav-structure.test.ts`.
 */
export function SettingsHome() {
  useEffect(warmSettingsReads, []);
  // Read once for the whole list: only the System row spends it.
  const news = systemAttentionBadge();
  return (
    <div class="content-view active settings-panel">
      {SETTINGS_NAV_ITEMS.map(({ key, label, group }, i) => (
        // System leads to both causes of the badge, so its row shows the union.
        <SettingsNavRow
          key={key}
          label={label}
          badge={key === 'system' ? news : null}
          onClick={() => openSettingsSubview(key)}
        >
          {/* Groups are contiguous, so a heading goes wherever the group changes. */}
          {group !== SETTINGS_NAV_ITEMS[i - 1]?.group && (
            <div class="settings-nav-group-title">{group}</div>
          )}
        </SettingsNavRow>
      ))}
    </div>
  );
}
