import { SETTINGS_SYSTEM_SUBPANEL_ITEMS } from '../../store/store';
import { openSettingsSubview } from '../../store/actions/menu';
import { settingsRowBadge } from '../../store/systemAttentionBadge';
import { SettingsNavRow } from './SettingsNavRow';

export function SystemSubmenu() {
  return (
    <>
      {SETTINGS_SYSTEM_SUBPANEL_ITEMS.map(({ key, label }) => (
        <SettingsNavRow
          key={key}
          label={label}
          badge={settingsRowBadge(key)}
          onClick={() => openSettingsSubview(key)}
        />
      ))}
    </>
  );
}
