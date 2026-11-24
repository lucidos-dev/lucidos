
export type WorkspaceLanding = 'notifications' | { thread: string };

export function isCanonicalThreadId(id: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(id);
}

export function landingHash(landing?: WorkspaceLanding): string {
  if (!landing) return '';
  return landing === 'notifications' ? '#notifications' : `#thread=${landing.thread}`;
}

export function landingName(landing: WorkspaceLanding): string {
  return landing === 'notifications' ? 'notifications' : `thread:${landing.thread}`;
}
