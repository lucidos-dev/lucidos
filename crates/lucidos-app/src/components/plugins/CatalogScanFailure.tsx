import { marketplaceCatalog, marketplaceScanning } from '../../store/store';
import { rescanPluginCatalogAction } from '../../store/actions/plugin-marketplaces';
import type { Loadable, MarketplaceCatalog } from '../../store/types';

/** What the scan-failure notice says, or `null` when the last scan worked. */
export interface ScanFailure {
  reason: string;
  /** A new scan is running, so the notice says so in place of its Try again. */
  retrying: boolean;
}

/** The notice's state, as a pure function of what the panel holds.
 *
 *  A failed scan freezes the list at its last result. The panel names the
 *  failure, so old rows never pass as current. The notice stays up through a
 *  retry: the scheduler retries every five minutes, and a notice that blinked
 *  out each time would move the list under the reader. */
export function catalogScanFailure(
  catalog: Loadable<MarketplaceCatalog>,
  scanning: boolean,
): ScanFailure | null {
  if (catalog.status !== 'loaded' || !catalog.data.scan_error) return null;
  return { reason: catalog.data.scan_error, retrying: scanning };
}

/** A notice above the plugin list, drawn only while the last scan failed. */
export function CatalogScanFailure() {
  const failure = catalogScanFailure(marketplaceCatalog.value, marketplaceScanning.value);
  if (!failure) return null;
  return (
    <p class="plugins-scan-failure error-text">
      Could not check the marketplaces: {failure.reason}.{' '}
      {failure.retrying ? (
        'Checking again…'
      ) : (
        <button type="button" class="accent-link" onClick={() => void rescanPluginCatalogAction()}>
          Try again
        </button>
      )}
    </p>
  );
}
