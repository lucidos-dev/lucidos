import { signal } from '@preact/signals';
import { gapRefreshedVersion } from './eventGap';

export const engineBuildLogOpen = signal(false);

export const engineBuildLogVersion = gapRefreshedVersion();
