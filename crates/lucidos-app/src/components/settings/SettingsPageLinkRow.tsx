import type { ComponentChildren } from 'preact';
import { openSettingsSubview } from '../../store/actions/menu';
import type { SettingsSubview } from '../../store/store';

/** A row on one settings page that leads to another: a label, and a button
 *  that opens `view`. */
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
