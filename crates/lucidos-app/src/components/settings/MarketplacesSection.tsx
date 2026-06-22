import { useEffect, useState } from 'preact/hooks';
import { marketplaceCatalog } from '../../store/store';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import {
  addPluginMarketplaceAction,
  loadPluginCatalog,
  removePluginMarketplaceAction,
} from '../../store/actions/plugin-marketplaces';
import { AddOfficialMarketplaceButton } from '../apps/AddOfficialMarketplaceButton';

/** Settings → Marketplaces. Add/remove the git repositories (plugin
 *  marketplaces) the Store scans for installable plugins. Moved here out of the
 *  Store toolbar so the Store itself is just a searchable plugin list. */
export function MarketplacesSection() {
  const loadable = marketplaceCatalog.value;
  const showLoading = useDelayedLoading(loadable);
  const [source, setSource] = useState('');
  const [name, setName] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => { void loadPluginCatalog(); }, []);

  async function addMarketplace(e: Event) {
    e.preventDefault();
    setSaving(true);
    try {
      if (await addPluginMarketplaceAction(source, name)) {
        setSource('');
        setName('');
      }
    } finally {
      setSaving(false);
    }
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title">Marketplaces</div>
      <p class="settings-section-desc">
        Git repositories the Store scans for installable plugins. The engine
        re-checks them periodically and notifies you when an installed plugin
        has an update.
      </p>

      <form class="app-store-marketplace-form" onSubmit={addMarketplace}>
        <input
          class="settings-text-input app-store-source-input"
          value={source}
          onInput={(e) => setSource((e.currentTarget as HTMLInputElement).value)}
          placeholder="Marketplace git URL"
          aria-label="Marketplace git URL"
          disabled={saving}
        />
        <input
          class="settings-text-input app-store-name-input"
          value={name}
          onInput={(e) => setName((e.currentTarget as HTMLInputElement).value)}
          placeholder="Name"
          aria-label="Marketplace name"
          disabled={saving}
        />
        <button class="action-btn" type="submit" disabled={saving || !source.trim()}>
          Add
        </button>
      </form>

      {loadable.status === 'failed' ? (
        <LoadableError noun="marketplaces" error={loadable.error} />
      ) : loadable.status !== 'loaded' ? (
        showLoading ? <div class="loading-spinner" /> : null
      ) : loadable.data.marketplaces.length === 0 ? (
        <div class="app-store-empty-suggest app-store-empty-suggest-settings">
          <div class="app-store-muted-row">No marketplaces registered.</div>
          <AddOfficialMarketplaceButton />
        </div>
      ) : (
        <div class="list-rows app-store-marketplaces">
          {loadable.data.marketplaces.map((marketplace) => (
            <div class="list-row app-store-marketplace-row" key={marketplace.id}>
              <div class="list-row-info">
                <div class="title list-row-name">{marketplace.name}</div>
                <code class="app-store-source-value" data-tooltip={marketplace.source}>
                  {marketplace.source}
                </code>
              </div>
              <div class="list-row-actions">
                <button
                  class="action-btn action-btn-danger"
                  type="button"
                  onClick={() => void removePluginMarketplaceAction(marketplace.id)}
                >
                  Remove
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      {loadable.status === 'loaded' && loadable.data.errors.length > 0 && (
        <div class="list-rows app-store-errors">
          {loadable.data.errors.map((issue) => (
            <div class="list-row app-store-error-row" key={`${issue.marketplace_id}-${issue.error}`}>
              <div class="list-row-info">
                <div class="title list-row-name">{issue.marketplace_name}</div>
                <div class="list-row-details">{issue.error}</div>
                <code class="app-store-source-value" data-tooltip={issue.source}>{issue.source}</code>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
