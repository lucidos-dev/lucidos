import { useState } from 'preact/hooks';
import { spendCapHolds, trySpendCapNow } from '../../store/actions/spendCapHolds';
import { spendCapResetLabel } from '../../utils/spendCap';

function holdFor(provider: string) {
  const state = spendCapHolds.value;
  return state.status === 'loaded' ? state.data.find((hold) => hold.provider === provider) : undefined;
}

export function ProviderSpendCapStatus({ provider }: { provider: string }) {
  const hold = holdFor(provider);
  return hold ? <p class="list-row-details list-row-details-prose">Spend cap reached. {spendCapResetLabel(hold.resets_at)}</p> : null;
}

export function ProviderSpendCapAction({ provider }: { provider: string }) {
  const [busy, setBusy] = useState(false);
  if (!holdFor(provider)) return null;
  return (
    <button type="button" class="action-btn action-btn-secondary" disabled={busy} onClick={() => {
      setBusy(true);
      void trySpendCapNow(provider).finally(() => setBusy(false));
    }}>Try now</button>
  );
}
