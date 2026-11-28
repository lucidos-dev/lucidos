import { marketplaceCatalog, marketplaceScanning } from '../../store/store';
import { rescanPluginCatalogAction } from '../../store/actions/plugin-marketplaces';
import type { Loadable, MarketplaceCatalog } from '../../store/types';

export interface ScanFailure {
  reason: string;
  retrying: boolean;
}

export function catalogScanFailure(
  catalog: Loadable<MarketplaceCatalog>,
  scanning: boolean,
): ScanFailure | null {
  if (catalog.status !== 'loaded' || !catalog.data.scan_error) return null;
  return { reason: catalog.data.scan_error, retrying: scanning };
}

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
