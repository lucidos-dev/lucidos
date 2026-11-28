export function isUnstampedBuildId(id: string): boolean {
  return id.startsWith('__');
}

export function formatBuildId(id: string): string {
  return isUnstampedBuildId(id) ? 'dev' : id;
}
