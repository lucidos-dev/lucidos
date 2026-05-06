import { request } from './_fetch';

export interface Thread {
  id: string;
  title: string;
  source: string;
  last_activity: string;
  message_count: number;
  is_saved: boolean;
  has_response: boolean;
}

export const threads = {
  list(): Promise<Thread[]> {
    return request<{ threads: Thread[] }>('/api/threads').then(r => r.threads);
  },

  search(query: string): Promise<Thread[]> {
    return request<{ threads: Thread[] }>(
      `/api/threads/search?q=${encodeURIComponent(query)}`
    ).then(r => r.threads);
  },
};
