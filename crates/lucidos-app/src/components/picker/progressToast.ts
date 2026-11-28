import { removeToast, showToast } from '../../store/store';

export async function withProgressToast<T>(key: string, message: string, work: Promise<T>): Promise<T> {
  showToast(message, 'info', { key, spinning: true, dismissable: false });
  try {
    return await work;
  } finally {
    removeToast(key);
  }
}
