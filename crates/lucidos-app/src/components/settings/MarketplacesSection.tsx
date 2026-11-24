import { useEffect, useState } from 'preact/hooks';
import { marketplaceCatalog } from '../../store/store';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useInlineRename } from '../../hooks/useInlineRename';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { LoadingFade } from '../shared/LoadingFade';
import { Explainer } from '../shared/Explainer';
import { EditIcon } from '../shared/icons';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import {
  addPluginMarketplaceAction,
  loadPluginCatalog,
  refreshPluginCatalog,
  removePluginMarketplaceAction,
  renamePluginMarketplaceAction,
} from '../../store/actions/plugin-marketplaces';
import { AddOfficialMarketplaceButton } from '../plugins/AddOfficialMarketplaceButton';
import type { PluginMarketplace } from '../../store/types';

function MarketplaceRow({ marketplace }: { marketplace?: PluginMarketplace }) {
  const sk = useSkeleton();
  const name = marketplace?.name ?? '';
  const { renaming, draft, setDraft, inputRef, open, commit, cancel } = useInlineRename(
    name,
    async (next) => {
      if (marketplace) await renamePluginMarketplaceAction(marketplace.source, next);
    },
  );

  return (
    <div class={`list-row app-store-marketplace-row${renaming ? ' app-store-marketplace-renaming' : ''}`}>
      <div class="list-row-info">
        <div class="app-store-marketplace-name-slot">
          <SkText class="title list-row-name" as="div" w="9rem">{name}</SkText>
          {!sk && (
            <input
              ref={inputRef}
              class="app-store-marketplace-name-input"
              type="text"
              value={draft}
              {...PROSE_TEXT_ATTRS}
              aria-label="Marketplace name"
              tabIndex={renaming ? 0 : -1}
              aria-hidden={!renaming}
              onInput={(e) => setDraft((e.currentTarget as HTMLInputElement).value)}
              onBlur={renaming ? commit : undefined}
              data-escape-self
              onKeyDown={(e) => {
                if (e.key === 'Enter') void commit();
                else if (e.key === 'Escape') { e.preventDefault(); cancel(); }
              }}
            />
          )}
        </div>
        {sk ? (
          <SkText class="app-store-source-value" as="div" w="16rem" />
        ) : (
          <code class="app-store-source-value" data-tooltip={marketplace?.source}>
            {marketplace?.source}
          </code>
        )}
      </div>
      <div class="list-row-actions">
        <SkBlock w="2.25rem" h="2.25rem" round>
          <button
            class="icon-btn row-icon app-store-marketplace-rename"
            type="button"
            onClick={open}
            aria-label={`Rename marketplace “${name}”`}
            data-tooltip="Rename"
          >
            <EditIcon />
          </button>
        </SkBlock>
        <SkBlock w="4.5rem" h="2rem" round>
          <button
            class="action-btn action-btn-danger"
            type="button"
            onClick={() => { if (marketplace) void removePluginMarketplaceAction(marketplace.id); }}
          >
            Remove
          </button>
        </SkBlock>
      </div>
    </div>
  );
}

export function MarketplacesSection() {
  usePanelRefresh('marketplaces', refreshPluginCatalog);
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
      <div class="settings-section-title">
        Marketplaces
        <Explainer title="Marketplaces">
          <p>Git repositories the Store scans for installable plugins.</p>
          <p>
            The engine re-checks them periodically and notifies you when an installed
            plugin has an update.
          </p>
          <p>
            A marketplace is its URL, so only the name can be edited. To point at
            another repository, remove the row and add the new URL.
          </p>
        </Explainer>
      </div>

      <form class="app-store-marketplace-form" onSubmit={addMarketplace}>
        <input
          class="settings-text-input app-store-source-input"
          value={source}
          onInput={(e) => setSource((e.currentTarget as HTMLInputElement).value)}
          placeholder="Marketplace git URL"
          aria-label="Marketplace git URL"
          disabled={saving}
        />
        <div class="app-store-marketplace-form-tail">
          <input
            class="settings-text-input app-store-name-input"
            value={name}
            {...PROSE_TEXT_ATTRS}
            onInput={(e) => setName((e.currentTarget as HTMLInputElement).value)}
            placeholder="Name"
            aria-label="Marketplace name"
            disabled={saving}
          />
          <button class="action-btn" type="submit" disabled={saving || !source.trim()}>
            Add
          </button>
        </div>
      </form>

      {loadable.status === 'failed' ? (
        <LoadableError noun="marketplaces" error={loadable.error} />
      ) : (
        <LoadingFade showSkeleton={showLoading} skeleton={<ListSkeletonOf count={2} containerClass="list-rows app-store-marketplaces" row={() => <MarketplaceRow />} />}>
          {loadable.status === 'loaded' ? (
            loadable.data.marketplaces.length === 0 ? (
              <div class="app-store-empty-suggest app-store-empty-suggest-settings">
                <div class="app-store-muted-row">No marketplaces registered.</div>
                <AddOfficialMarketplaceButton />
              </div>
            ) : (
              <div class="list-rows app-store-marketplaces">
                {loadable.data.marketplaces.map((marketplace) => (
                  <MarketplaceRow key={marketplace.id} marketplace={marketplace} />
                ))}
              </div>
            )
          ) : null}
        </LoadingFade>
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
