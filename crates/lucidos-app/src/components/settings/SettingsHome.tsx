import { useEffect } from 'preact/hooks';
import { SETTINGS_NAV_ITEMS } from '../../store/store';
import { warmSettingsReads } from '../../store/actions/settingsReads';
import { systemAttentionBadge } from '../../store/systemAttentionBadge';
import { openSettingsSubview } from '../../store/actions/menu';
import { SettingsNavRow } from './SettingsNavRow';

export function SettingsHome() {
  useEffect(warmSettingsReads, []);
  const news = systemAttentionBadge();
  return (
    <div class="content-view active settings-panel">
      {SETTINGS_NAV_ITEMS.map(({ key, label, group }, i) => (
        <SettingsNavRow
          key={key}
          label={label}
          badge={key === 'system' ? news : null}
          onClick={() => openSettingsSubview(key)}
        >
          {group !== SETTINGS_NAV_ITEMS[i - 1]?.group && (
            <div class="settings-nav-group-title">{group}</div>
          )}
        </SettingsNavRow>
      ))}
    </div>
  );
}
