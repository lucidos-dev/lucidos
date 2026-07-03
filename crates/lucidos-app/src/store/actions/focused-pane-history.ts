// Focused-pane-aware history navigation. The Back/Forward keyboard shortcuts
// (historyBack / historyForward) act on whichever pane currently holds focus,
// routing to that pane's OWN history stack:
//   - content pane → the panel nav stack (navigation.ts)
//   - thread / drawer pane → the thread nav stack (thread-navigation.ts)
// The dedicated Back/Forward buttons in each pane header still drive their own
// stack directly; this dispatcher exists only for the shared keyboard shortcut.
//
// Desktop-only by nature: `focusedPane` never leaves its default on mobile
// (focusPane is a no-op there) and mobile has no keyboard shortcuts — the mobile
// header's per-pane buttons cover navigation instead.

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
