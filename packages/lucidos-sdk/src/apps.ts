import { request } from './_fetch';
import type { WidgetSize } from './widgetFrame';

export interface App {
  id: string;
  name: string;
  description: string;
  icon?: string;
  reveal: 'on-load' | 'on-ready';
  kind: 'app' | 'widget';
  origin_thread_id?: string;
  origin_plugin_id?: string;
  reusable: boolean;
  params?: Record<string, { description: string; required?: boolean }>;
  built_in?: boolean;
  sizes?: WidgetSize[];
  transparent?: boolean;
}

export const apps = {
  list(): Promise<App[]> {
    return request('/apps');
  },

  get(id: string): Promise<App> {
    return request(`/app?id=${encodeURIComponent(id)}`);
  },
};
