import { SETTINGS_SYSTEM_SUBPANEL_ITEMS } from '../../store/store';
import { openSettingsSubview } from '../../store/actions/menu';
import { systemPageBadge } from '../../store/systemAttentionBadge';
import { SettingsNavRow } from './SettingsNavRow';

/**
 * Settings > System: the list of its sub-pages.
 *
 * The same drilldown the Settings home is, one level down, sharing its row.
 * Each sub-page therefore renders alone, with no chrome above it. Overview is
 * a row here like any other, which is what `system-overview` exists for.
 */
export function SystemSubmenu() {
  return (
    <>
      {SETTINGS_SYSTEM_SUBPANEL_ITEMS.map(({ key, label }) => (
        // BY SOURCE, never the union: this is the last step of the path, so a
        // mark here promises work on the page the row opens. An update dotting
        // Release Notices would send the reader somewhere with nothing on it.
        <SettingsNavRow
          key={key}
          label={label}
          badge={systemPageBadge(key)}
          onClick={() => openSettingsSubview(key)}
        />
      ))}
    </>
  );
}
