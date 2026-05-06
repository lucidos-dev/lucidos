import { request } from './_fetch';

export interface App {
  id: string;
  name: string;
  description: string;
  icon?: string;
  knowhow: string[];
}

export const apps = {
  list(): Promise<App[]> {
    return request('/api/apps');
  },

  get(id: string): Promise<App> {
    return request(`/api/app?id=${encodeURIComponent(id)}`);
  },
};
