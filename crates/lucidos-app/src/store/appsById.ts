/** Apps read one by one, for ids the apps list does not hold. A widget is
 *  such an id (ADR 0402): the apps list never holds it, so a reader that must
 *  name one reads it here. `null` means the engine answered 404: the app is
 *  gone. */

import { signal } from '@preact/signals';
import type { App, Loadable } from './types';

export const appsById = signal<ReadonlyMap<string, Loadable<App | null>>>(new Map());

export function appById(id: string): Loadable<App | null> {
  return appsById.value.get(id) ?? { status: 'not-loaded' };
}
