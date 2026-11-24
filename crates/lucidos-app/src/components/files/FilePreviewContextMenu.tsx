import { useRef } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { OverflowMenu, type OverflowMenuOpener } from '../shared/OverflowMenu';
import { renderMenuAction } from '../layout/headerActions';
import { filePreviewContentActions } from '../layout/ContentHeaderActions';
import { shouldOpenFilePreviewMenu } from './filePreviewContextMenuPolicy';
import { selectionCovers, opensContextMenu } from '@lucidos/context-menu';

export function FilePreviewContextMenu({ path, layout, children }: {
  path: string;
  layout: 'desktop' | 'mobile';
  children: ComponentChildren;
}) {
  const openRef = useRef<OverflowMenuOpener | null>(null);
  const selectionAtPress = useRef(false);
  const desktop = layout === 'desktop';

  const onPointerDownCapture = (e: PointerEvent) => {
    if (!opensContextMenu(e)) return;
    selectionAtPress.current = selectionCovers(window.getSelection(), { x: e.clientX, y: e.clientY });
  };

  const onContextMenu = (e: MouseEvent) => {
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
