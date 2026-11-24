import { activeExchanges, showToast } from '../../store/store';
import { exchangeResponseText, type Exchange } from '../../store/thread-events';
import { copyToClipboard } from '../../utils/clipboard';

export function lastResponseText(exchanges: readonly Exchange[]): string | null {
  for (let i = exchanges.length - 1; i >= 0; i--) {
    const text = exchangeResponseText(exchanges[i]).trim();
    if (text) return text;
  }
  return null;
}

export function copyLastResponse(): void {
  const text = lastResponseText(activeExchanges.value);
  if (text === null) {
    showToast('This thread has no response to copy yet.', 'info');
    return;
  }
  copyToClipboard(text, 'Response copied');
}
