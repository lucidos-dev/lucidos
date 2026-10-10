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

/** Find the nearest ancestor of `target` marked as a drop zone. The innermost
 *  match wins, so a files panel nested inside an attach surface still imports
 *  cleanly. Returns the matched element so callers can apply visual feedback. */
export function findDropZone(target: EventTarget | null): DropZoneMatch | null {
  const el = (target as HTMLElement | null)?.closest?.(ZONE_SELECTOR) as HTMLElement | null;
  if (!el) return null;
  const kind = el.getAttribute('data-drop-zone') as DropZoneKind;
  return { el, kind };
}

/** Resolve the drop target and invoke the matching handler. Returns the kind
 *  that was dispatched, or null if the drop should be ignored. Caller must
 *  call `preventDefault` on the underlying event. */
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
