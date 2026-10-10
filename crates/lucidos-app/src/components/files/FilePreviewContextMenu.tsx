import { useRef } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { OverflowMenu, type OverflowMenuOpener } from '../shared/OverflowMenu';
import { renderMenuAction } from '../layout/headerActions';
import { filePreviewContentActions } from '../layout/ContentHeaderActions';
import { shouldOpenFilePreviewMenu } from './filePreviewContextMenuPolicy';
import { selectionCovers, opensContextMenu } from '../../utils/nativeContextMenu';

/** Wraps the file-preview body in a right-click menu. It offers the SAME
 *  actions the header toolbar does (`filePreviewContentActions`), so the two
 *  can never drift (ADR 0285; see
 *  `docs/plans/2026-10-04-file-preview-context-menu.md`).
 *
 *  Desktop only. Touch keeps using the existing header toolbar. Reusing the
 *  app's long-press gesture over real, selectable content risks breaking
 *  native text selection on iOS/Android. The toolbar already carries every
 *  one of these actions there.
 *
 *  Renders `.file-preview-frame-body` itself, with the same class and
 *  position `ContentPane.tsx` gave it, so adopting this needs no CSS
 *  change. */
export function FilePreviewContextMenu({ path, layout, children }: {
  path: string;
  layout: 'desktop' | 'mobile';
  children: ComponentChildren;
}) {
  const openRef = useRef<OverflowMenuOpener | null>(null);
  // Whether a selection covered the point BEFORE this press, snapshotted
  // here rather than read at `contextmenu` time.
  //
  // WebKit auto-selects the word under a right-click on the way there. So
  // reading the selection at event time can no longer tell the two apart
  // (same technique as `nativeContextMenu.ts`'s global policy).
  const selectionAtPress = useRef(false);
  const desktop = layout === 'desktop';

  const onPointerDownCapture = (e: PointerEvent) => {
    if (!opensContextMenu(e)) return;
    selectionAtPress.current = selectionCovers(window.getSelection(), { x: e.clientX, y: e.clientY });
  };

  const onContextMenu = (e: MouseEvent) => {
    // Consumed on read: a menu with no matching press (Shift+F10, VoiceOver)
    // must not inherit a flag an EARLIER, unrelated right-click left set.
    const heldSelection = selectionAtPress.current;
    selectionAtPress.current = false;
    if (!shouldOpenFilePreviewMenu(e, heldSelection)) return;
    const actions = filePreviewContentActions(path, false);
    if (actions.length === 0) return;
    e.preventDefault();
    openRef.current?.(e.currentTarget as HTMLElement, { x: e.clientX, y: e.clientY });
  };

  return (
    <div
      class="file-preview-frame-body"
      onPointerDownCapture={desktop ? onPointerDownCapture : undefined}
      onContextMenu={desktop ? onContextMenu : undefined}
    >
      {children}
      {desktop && (
        <OverflowMenu
          ariaLabel="File actions"
          hostOpener={{ ref: openRef, trigger: false }}
          items={(ctx) => filePreviewContentActions(path, false).map((a) => renderMenuAction(a, ctx))}
        />
      )}
    </div>
  );
}
