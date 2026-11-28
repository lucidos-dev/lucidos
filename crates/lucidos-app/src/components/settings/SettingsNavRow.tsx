import type { ComponentChildren } from 'preact';
import { SystemAttentionBadge } from '../shared/SystemAttentionBadge';
import { ChevronRightIcon } from '../shared/icons';

export function SettingsNavRow(
  { label, badge = null, onClick, children }: {
    label: string;
    badge?: string | null;
    onClick: () => void;
    children?: ComponentChildren;
  },
) {
  return (
    <div class="settings-section settings-nav-item">
      {children}
      <button
        type="button"
        class="settings-section-title settings-nav-row"
        aria-label={badge ? `${label} · ${badge}` : undefined}
        onClick={onClick}
      >
        <span>{label}<SystemAttentionBadge placement="inline" label={badge} /></span>
        <ChevronRightIcon />
      </button>
    </div>
  );
}
