
import { signal } from '@preact/signals';
import type { App, Loadable } from './types';

export const appsById = signal<ReadonlyMap<string, Loadable<App | null>>>(new Map());

export function appById(id: string): Loadable<App | null> {
  return appsById.value.get(id) ?? { status: 'not-loaded' };
}
