import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { getRemPx } from '../../utils/dom';
import { startDividerDrag } from '../layout/dividerDrag';
import {
  repoPreviewSidebarRem, setSidebarRem, clampSidebarRem, sidebarMaxRem,
  SIDEBAR_DEFAULT_REM, SIDEBAR_MIN_REM, SIDEBAR_KEY_STEP_REM,
} from './repoPreviewSidebarWidth';

interface Props {
  sidebar: ComponentChildren;
  main: ComponentChildren;
}

/** The repo file preview's two columns: the changed-files sidebar and the main
 *  pane, with a divider between them that resizes the sidebar. The container
 *  query in panels/content.css hides the sidebar and divider together on a
 *  narrow pane, so a phone never sees either. */
export function RepoPreviewSplit({ sidebar, main }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const asideRef = useRef<HTMLElement>(null);
  const [containerRem, setContainerRem] = useState(0);
  const endDrag = useRef<(() => void) | null>(null);

  // Observes the sidebar as well as the container: a UI-scale change resizes
  // the rem-sized sidebar but not the pane, and the rem width must follow it.
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const measure = () => setContainerRem(container.getBoundingClientRect().width / getRemPx());
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    if (asideRef.current) ro.observe(asideRef.current);
    return () => ro.disconnect();
  }, []);

  useEffect(() => () => endDrag.current?.(), []);

  const widthRem = clampSidebarRem(repoPreviewSidebarRem.value, containerRem);
  const maxRem = sidebarMaxRem(containerRem);

  const onPointerDown = (e: PointerEvent) => {
    const container = containerRef.current;
    if (e.button !== 0 || !container) return;
    e.preventDefault();
    const remPx = getRemPx();
    endDrag.current = startDividerDrag(
      e,
      (ev) => {
        const rect = container.getBoundingClientRect();
        repoPreviewSidebarRem.value = clampSidebarRem((ev.clientX - rect.left) / remPx, rect.width / remPx);
      },
      () => {
        endDrag.current = null;
        setSidebarRem(repoPreviewSidebarRem.value);
      },
    );
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const target =
      e.key === 'ArrowLeft' ? widthRem - SIDEBAR_KEY_STEP_REM
      : e.key === 'ArrowRight' ? widthRem + SIDEBAR_KEY_STEP_REM
      : e.key === 'Home' ? SIDEBAR_MIN_REM
      : e.key === 'End' ? maxRem
      : null;
    if (target === null) return;
    e.preventDefault();
    setSidebarRem(clampSidebarRem(target, containerRem));
  };

  return (
    <div class="repo-preview-split" ref={containerRef}>
      <aside class="repo-preview-split-sidebar" ref={asideRef} style={{ flexBasis: `${widthRem}rem` }}>
        {sidebar}
      </aside>
      <div
        class="repo-preview-split-divider"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize changed files sidebar"
        aria-valuenow={Math.round(widthRem)}
        aria-valuemin={SIDEBAR_MIN_REM}
        aria-valuemax={Math.round(maxRem)}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onKeyDown={onKeyDown}
        onDblClick={() => setSidebarRem(SIDEBAR_DEFAULT_REM)}
      />
      <div class="repo-preview-split-main">{main}</div>
    </div>
  );
}
