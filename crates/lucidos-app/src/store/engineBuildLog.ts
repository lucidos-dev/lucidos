import { signal } from '@preact/signals';
import { gapRefreshedVersion } from './eventGap';

/** Window-local visibility of the authenticated build-log viewer. */
export const engineBuildLogOpen = signal(false);

/** Build-state events invalidate a diagnostic snapshot in an open viewer. */
export const engineBuildLogVersion = gapRefreshedVersion();
