

export function replaceDocument(href: string): void {
  window.location.replace(href);
}

export interface ClickModifiers {
  button?: number;
  metaKey?: boolean;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  defaultPrevented?: boolean;
}

export function isPlainClick(e: ClickModifiers): boolean {
  if (e.defaultPrevented) return false;
  if (e.button !== undefined && e.button !== 0) return false;
  return !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey;
}

export function replaceOnPlainClick(href: string, before?: () => void) {
  return (e: MouseEvent): void => {
    before?.();
    if (!isPlainClick(e)) return;
    e.preventDefault();
    replaceDocument(href);
  };
}
