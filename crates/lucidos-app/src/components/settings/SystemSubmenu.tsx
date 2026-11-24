import { SETTINGS_SYSTEM_SUBPANEL_ITEMS } from '../../store/store';
import { openSettingsSubview } from '../../store/actions/menu';
import { systemPageBadge } from '../../store/systemAttentionBadge';
import { SettingsNavRow } from './SettingsNavRow';

export function SystemSubmenu() {
  return (
    <>
      {SETTINGS_SYSTEM_SUBPANEL_ITEMS.map(({ key, label }) => (
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
