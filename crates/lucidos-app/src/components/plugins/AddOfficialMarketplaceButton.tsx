import { useState } from 'preact/hooks';
import { addOfficialMarketplaceAction } from '../../store/actions/plugin-marketplaces';

/** One-click button that registers the official Lucidos plugin marketplace.
 *  Rendered in the Plugins panel catalog and Settings → Marketplaces empty states so a fresh
 *  workspace can start installing plugins without first finding a marketplace
 *  URL. The action toasts + re-scans the catalog, which then replaces the empty
 *  state with the marketplace's plugins. */
export function AddOfficialMarketplaceButton() {
  const [adding, setAdding] = useState(false);

  async function add() {
    setAdding(true);
    try {
      await addOfficialMarketplaceAction();
    } finally {
      setAdding(false);
    }
  }

  return (
    <button
      class="action-btn"
      type="button"
      disabled={adding}
      onClick={() => void add()}
    >
      {adding ? 'Adding…' : 'Add the official Lucidos marketplace'}
    </button>
  );
}
