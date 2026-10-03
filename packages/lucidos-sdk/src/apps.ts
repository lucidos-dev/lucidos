import { request } from './_fetch';

export interface App {
  id: string;
  name: string;
  description: string;
  icon?: string;
  /** When the host lifts its loading cover, from the manifest's `reveal`.
   *  `on-ready` waits for `lucidos.ui.ready()`. */
  reveal: 'on-load' | 'on-ready';
}

export const apps = {
  list(): Promise<App[]> {
    return request('/apps');
  },

  get(id: string): Promise<App> {
    return request(`/app?id=${encodeURIComponent(id)}`);
  },
};
