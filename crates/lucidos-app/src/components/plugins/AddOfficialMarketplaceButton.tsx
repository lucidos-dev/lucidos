import { useState } from 'preact/hooks';
import { addOfficialMarketplaceAction } from '../../store/actions/plugin-marketplaces';

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
