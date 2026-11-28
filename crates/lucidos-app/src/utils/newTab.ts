
export function openNewTab(url: string, target = '_blank'): boolean {
  const opened = window.open(url, target);
  if (!opened || opened.closed) return false;
  if (target === '_blank') opened.opener = null;
  return true;
}
