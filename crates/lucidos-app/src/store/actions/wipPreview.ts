import { effect } from '@preact/signals';
import { focusedThreadId, wipPreviewThreadId, threadMap, currentApp, threadsLoaded } from '../store';


effect(() => {
  const tid = wipPreviewThreadId.value;
  if (!tid) return;
  const focused = focusedThreadId.value;
  const map = threadMap.value;
  const loaded = threadsLoaded.value;
  if (focused !== tid) {
    wipPreviewThreadId.value = null;
    return;
  }
  const t = map.get(tid);
  if (!t) {
    if (loaded) wipPreviewThreadId.value = null;
    return;
  }
  const folder = t.meta.codingAgentFolder;
  const wipAppId = folder ? folder.split('/').filter(Boolean).pop() : undefined;
  const openApp = currentApp.value;
  if (openApp && wipAppId && openApp.id !== wipAppId) {
    wipPreviewThreadId.value = null;
  }
});

export function clearWipIfMatches(predicate: (currentWipThreadId: string) => boolean): void {
  const tid = wipPreviewThreadId.value;
  if (tid && predicate(tid)) {
    wipPreviewThreadId.value = null;
  }
}
