export type DropZoneKind = 'attach' | 'import';

export interface DropZoneMatch {
  el: HTMLElement;
  kind: DropZoneKind;
}

export interface DropHandlers {
  attach: (files: FileList) => Promise<void> | void;
  import: (files: FileList) => Promise<void> | void;
}

const ZONE_SELECTOR = '[data-drop-zone="attach"], [data-drop-zone="import"]';

export function findDropZone(target: EventTarget | null): DropZoneMatch | null {
  const el = (target as HTMLElement | null)?.closest?.(ZONE_SELECTOR) as HTMLElement | null;
  if (!el) return null;
  const kind = el.getAttribute('data-drop-zone') as DropZoneKind;
  return { el, kind };
}

export async function dispatchDrop(
  target: EventTarget | null,
  files: FileList | null | undefined,
  handlers: DropHandlers,
): Promise<DropZoneKind | null> {
  if (!files || files.length === 0) return null;
  const zone = findDropZone(target);
  if (!zone) return null;
  if (zone.kind === 'attach') {
    await handlers.attach(files);
  } else {
    await handlers.import(files);
  }
  return zone.kind;
}
