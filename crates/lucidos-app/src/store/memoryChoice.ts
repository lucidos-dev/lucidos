/**
 * Should the *memory choice* be up? A read over signals, for the App slot.
 *
 * It asks a workspace that has never chosen its memory module, on every
 * device. Every answer writes `memory_module`, closing the modal included, so
 * the stored choice is the only state: a peer's answer arrives as a
 * `PreferencesChanged` frame and closes the modal here too.
 */
import { storedMemoryModule } from './actions/preferences';
import { releaseNoticeModalOpen } from './releaseNotices';
import { engineStartedAt, llmConfigured, preferences, releaseNoticeView } from './store';

export function memoryChoiceOpen(): boolean {
  if (preferences.value.status !== 'loaded' || storedMemoryModule() !== null) return false;
  // Provider first: Tree's price depends on the compactor model a provider
  // serves. The health probe says whether one is set up, and has not answered
  // while the engine's start time is unknown.
  if (engineStartedAt.value === null || !llmConfigured.value) return false;
  // One modal at a time. A notice owed on this open goes first, so wait until
  // the list has answered, or failed to.
  const notices = releaseNoticeView.value.status;
  if (notices === 'not-loaded' || notices === 'loading') return false;
  return !releaseNoticeModalOpen();
}
