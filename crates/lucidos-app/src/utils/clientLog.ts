
import { API } from './basePath';
import { sourceGeneration } from './restoreSourceGeneration';

export function postClientLog(category: string, message: string, data: Record<string, unknown>): void {
  try {
    const generation = sourceGeneration();
    fetch(`${API}/internal/client-log`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'x-lucidos-restore-generation': generation ?? 'none' },
      body: JSON.stringify({ category, code: message, message, data }),
      keepalive: true,
    }).catch(() => {  });
  } catch {
  }
}
