import { marketplaceCatalog, marketplaceScanning } from '../../store/store';
import { rescanPluginCatalogAction } from '../../store/actions/plugin-marketplaces';
import { formatAgoPhrase } from '../../utils/formatTime';
import type { Loadable, MarketplaceCatalog } from '../../store/types';

/** What the freshness control says right now. */
export interface FreshnessState {
  label: string;
  tooltip: string;
  /** A scan is running, so this is a status rather than a button. */
  busy: boolean;
}

/** The control's whole state, as a pure function of what the panel holds.
 *
 *  The plugin list comes from a cache the engine refreshes on a timer, so it
 *  can be minutes old. Showing rows without saying how old they are is the
 *  dishonest half of stale-while-revalidate. This is the honest half.
 *
 *  `null` means draw nothing: with no catalog loaded the panel is showing a
 *  skeleton, and an age beside it would describe data nobody can see. */
export function catalogFreshness(
  catalog: Loadable<MarketplaceCatalog>,
  scanning: boolean,
  now: Date,
): FreshnessState | null {
  if (catalog.status !== 'loaded') return null;
  if (scanning) {
    return {
      label: 'Updating…',
      tooltip: 'Checking every marketplace for new and updated plugins.',
      busy: true,
    };
  }
  const { scanned_at, scan_error } = catalog.data;
  if (scan_error) {
    // Never silent. A scan that could not run at all leaves the list frozen,
    // and the user is owed the reason rather than a quietly ageing timestamp.
    return {
      label: 'Update failed',
      tooltip: `The last check could not run: ${scan_error}. Select to try again.`,
      busy: false,
    };
  }
  const scanned = scanned_at ? new Date(scanned_at) : null;
  if (!scanned || Number.isNaN(scanned.getTime())) {
    return {
      label: 'Not checked yet',
      tooltip: 'No marketplace has been checked yet. Select to check now.',
      busy: false,
    };
  }
  return {
    label: `Updated ${formatAgoPhrase(scanned, now)}`,
    tooltip: 'Select to check every marketplace for new and updated plugins.',
    busy: false,
  };
}

/** How old the plugin list is, and a way to refresh it.
 *
 *  One element in two states, not two elements: while a scan runs it is a
 *  status, and otherwise it is the button that starts one. */
export function CatalogFreshness() {
  const state = catalogFreshness(marketplaceCatalog.value, marketplaceScanning.value, new Date());
  if (!state) return null;
  if (state.busy) {
    return (
      <span class="plugins-freshness plugins-freshness-busy" data-tooltip={state.tooltip}>
        {state.label}
      </span>
    );
  }
  return (
    <button
      type="button"
      class="plugins-freshness"
      data-tooltip={state.tooltip}
      onClick={() => void rescanPluginCatalogAction()}
    >
      {state.label}
    </button>
  );
}
