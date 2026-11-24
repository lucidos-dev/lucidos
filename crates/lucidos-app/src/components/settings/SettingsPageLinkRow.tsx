import type { ComponentChildren } from 'preact';
import { openSettingsSubview } from '../../store/actions/menu';
import type { SettingsSubview } from '../../store/store';

export function SettingsPageLinkRow({ view, button, children }: {
  view: Exclude<SettingsSubview, 'main'>;
  button: string;
  children: ComponentChildren;
}) {
  return (
    <div class="settings-row">
      <span class="settings-row-label">{children}</span>
      <button class="action-btn" onClick={() => openSettingsSubview(view)}>
        {button}
      </button>
    </div>
  );
}
