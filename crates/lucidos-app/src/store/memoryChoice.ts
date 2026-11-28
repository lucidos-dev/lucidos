import { storedMemoryModule } from './actions/preferences';
import { releaseNoticeModalOpen } from './releaseNotices';
import { engineStartedAt, llmConfigured, preferences, releaseNoticeView } from './store';

export function memoryChoiceOpen(): boolean {
  if (preferences.value.status !== 'loaded' || storedMemoryModule() !== null) return false;
  if (engineStartedAt.value === null || !llmConfigured.value) return false;
  const notices = releaseNoticeView.value.status;
  if (notices === 'not-loaded' || notices === 'loading') return false;
  return !releaseNoticeModalOpen();
}
