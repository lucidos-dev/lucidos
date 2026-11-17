import { signal } from '@preact/signals';
import { clearSpendCapHold, getSpendCapHolds, type SpendCapHold } from '../../api/client/spendCapHolds';
import { failedIfFresh, setLoadingIfFresh, type Loadable } from '../types';
import { showToast } from '../store';
import { createFailureCounter } from '../../utils/failureCounter';
import { errorDetail } from '../../utils/errorDetail';

export const spendCapHolds = signal<Loadable<SpendCapHold[]>>({ status: 'not-loaded' });
let sequence = 0;
const failures = createFailureCounter(3, () => showToast('Could not refresh provider spend-cap status.', 'error'));

export async function loadSpendCapHolds(): Promise<void> {
  const current = ++sequence;
  setLoadingIfFresh(spendCapHolds);
  try {
    const holds = await getSpendCapHolds();
    if (current === sequence) {
      failures.recordSuccess();
      spendCapHolds.value = { status: 'loaded', data: holds };
    }
  } catch (error) {
    if (current === sequence) {
      console.warn('[spend-cap] Status refresh failed', error);
      failures.recordFailure();
      spendCapHolds.value = failedIfFresh(spendCapHolds.value, error);
    }
  }
}

export async function trySpendCapNow(provider: string): Promise<void> {
  try {
    await clearSpendCapHold(provider);
    await loadSpendCapHolds();
  } catch (error) {
    showToast(`Could not clear the spend-cap hold: ${errorDetail(error)}`, 'error');
  }
}
