import { signal } from '@preact/signals';
import { isTauri } from '../utils/platform';

export const enginePackaged = signal<boolean | null>(null);

export function restartRestartsGateway(): boolean {
  return enginePackaged.value === true && isTauri();
}
