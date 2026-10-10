import { request } from './_fetch';

export interface App {
  id: string;
  name: string;
  description: string;
  icon?: string;
  /** When the host lifts its loading cover, from the manifest's `reveal`.
   *  `on-ready` waits for `lucidos.ui.ready()`. */
  reveal: 'on-load' | 'on-ready';
  /** `widget` for a small answer shown inline in a thread (ADR 0402).
   *  `list()` returns apps only; `get()` answers for a widget too. */
  kind: 'app' | 'widget';
  /** The thread a widget was made in. Omitted for an app. */
  origin_thread_id?: string;
  /** A reusable widget may be shown in any thread. Always false for an app. */
  reusable: boolean;
  /** The names a widget takes in its params (ADR 0415). Omitted when it
   *  declares none. */
  params?: Record<string, { description: string; required?: boolean }>;
  /** A built-in widget: shipped with Lucidos and read-only. Omitted otherwise. */
  built_in?: boolean;
}

export const apps = {
  list(): Promise<App[]> {
    return request('/apps');
  },

  get(id: string): Promise<App> {
    return request(`/app?id=${encodeURIComponent(id)}`);
  },
};
