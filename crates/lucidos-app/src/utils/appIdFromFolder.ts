export function appIdFromFolder(folder: string | null | undefined): string | null {
  if (!folder) return null;
  const segments = folder.split('/').filter(s => s.length > 0);
  const appsIdx = segments.lastIndexOf('apps');
  if (appsIdx === -1 || appsIdx === segments.length - 1) return null;
  if (appsIdx === 0 || segments[appsIdx - 1] !== 'data') return null;
  const id = segments[appsIdx + 1];
  if (!id || id === '.' || id === '..') return null;
  return id;
}
