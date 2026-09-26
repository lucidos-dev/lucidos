import { useEffect, useState } from 'preact/hooks';
import { marketplaceCatalog } from '../../store/store';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
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
  removePluginMarketplaceAction,
  renamePluginMarketplaceAction,
} from '../../store/actions/plugin-marketplaces';
import { AddOfficialMarketplaceButton } from '../plugins/AddOfficialMarketplaceButton';
import type { PluginMarketplace } from '../../store/types';

/** Self-skeletonizing marketplace row: rendered with no props inside a
 *  SkeletonProvider (`<MarketplaceRow />`) it draws itself as a loading
 *  placeholder via the Sk* leaves; with a real `marketplace` it renders normally.
 *  Props are optional only to support the skeleton call.
 *
 *  The name renames in place. The URL below it is read-only, because it is the
 *  marketplace's identity: the engine keys the entry on a hash of it, so a
 *  changed URL is a different marketplace. Point somewhere else with Remove
 *  plus the Add form above. */
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
          {/* Mounted whether or not we are renaming, invisible and pointer-inert
              over the name until then. iOS raises the keyboard only for a
              focus() inside the user's gesture, and a field the tap renders does
              not exist yet at that moment. The pencil focuses this one. */}
          {!sk && (
            <input
              ref={inputRef}
              class="app-store-marketplace-name-input"
              type="text"
              value={draft}
              {...PROSE_TEXT_ATTRS}
              aria-label="Marketplace name"
              tabIndex={renaming ? 0 : -1}
              // Exposed exactly while the field is real. Idle, it is a textbox
              // that does nothing, and it reads the name out a second time.
              aria-hidden={!renaming}
              onInput={(e) => setDraft((e.currentTarget as HTMLInputElement).value)}
              onBlur={renaming ? commit : undefined}
              // The blur commits, so the central Escape policy must not blur it.
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
        {/* A display name is prose, unlike the URL beside it: the keyboard's
            own capitalization is what a name wants, and an autocapitalized URL
            is a broken one. */}
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
