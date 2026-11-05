import type { ComponentChildren } from 'preact';
import { SystemAttentionBadge } from '../shared/SystemAttentionBadge';
import { ChevronRightIcon } from '../shared/icons';

/**
 * One row of a Settings drilldown list: a label, an optional attention mark,
 * and the chevron saying it leads somewhere.
 *
 * Both lists render it, the Settings home and the System submenu, so the
 * accessibility contract below is stated once instead of per list.
 *
 * `badge` is the sentence the mark stands for, or null for no mark. The list
 * decides which read to pass: the home row passes the union, because it leads
 * to both causes, and a System row passes its own page's half.
 */
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
      {/* A real <button>, not a clickable div: a row is the only way into what
          it opens, so a div puts that page, and every control on it, out of
          keyboard reach. */}
      <button
        type="button"
        class="settings-section-title settings-nav-row"
        // The mark is decorative, so the row says the words. Only a badged row
        // carries a label at all: everywhere else the visible text already
        // names the row, and repeating it would be noise.
        aria-label={badge ? `${label} · ${badge}` : undefined}
        onClick={onClick}
      >
        {/* Inside the label span, so the mark hugs the word and the chevron
            keeps the row's trailing edge. */}
        <span>{label}<SystemAttentionBadge placement="inline" label={badge} /></span>
        <ChevronRightIcon />
      </button>
    </div>
  );
}
