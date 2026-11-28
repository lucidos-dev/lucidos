
import { focusedPane } from '../store';
import { navBack, navForward } from './navigation';
import { threadNavBack, threadNavForward } from './thread-navigation';

export function historyBack(): void {
  if (focusedPane.value === 'content') navBack();
  else threadNavBack();
}

export function historyForward(): void {
  if (focusedPane.value === 'content') navForward();
  else threadNavForward();
}
