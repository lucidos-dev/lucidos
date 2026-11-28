
export function withPreviewRevision(url: string, rev: number): string {
  if (!rev) return url;
  return `${url}${url.includes('?') ? '&' : '?'}v=${rev}`;
}
