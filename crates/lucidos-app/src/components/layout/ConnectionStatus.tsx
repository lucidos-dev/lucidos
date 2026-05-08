import { useRef, useState, useLayoutEffect } from 'preact/hooks';
import { connectionStatus, workspaceName } from '../../store/store';
import { getRemPx } from '../../utils/dom';

export function ConnectionStatus() {
  const status = connectionStatus.value;
  const name = workspaceName.value;
  const labelRef = useRef<HTMLSpanElement>(null);
  const [hidden, setHidden] = useState(false);

  useLayoutEffect(() => {
    if (!name || !labelRef.current) return;
    const label = labelRef.current;
    const brandLabel = label.closest('.pane-header-brand-label') as HTMLElement | null;
    const brand = brandLabel?.closest('.pane-header-brand') as HTMLElement | null;
    const row = brand?.parentElement;
    if (!brandLabel || !brand || !row) return;
    // Mobile: brand shares row width with .pane-header-spacer. Desktop: brand
    // has fixed width and no spacer sibling, so spacer.clientWidth defaults to 0.
    const spacer = row.querySelector(':scope > .pane-header-spacer') as HTMLElement | null;

    // Mirrors .workspace-name-label margin-left in panels.css. is-hidden zeros
    // it (so hiding frees the gap), so the live computed margin can't be read
    // back once hidden — the constant keeps the threshold invariant.
    const wsMarginPx = 0.35 * getRemPx();

    const update = () => {
      // Workspace absorbs all flex shrink first via the :has() rule in
      // panels.css that pins lucidos to flex-shrink:0 while the label is
      // visible. Only flip is-hidden when even workspace shrunk to its margin
      // gap can't fit — i.e. when lucidos+dot natural already overflow.
      let nonWorkspace = 0;
      for (const child of Array.from(brandLabel.children) as HTMLElement[]) {
        if (child === label) continue;
        const cs = getComputedStyle(child);
        nonWorkspace += child.scrollWidth + (parseFloat(cs.marginLeft) || 0) + (parseFloat(cs.marginRight) || 0);
      }
      // brandLabel.clientWidth is the usable area inside brand (skips brand's
      // padding-left for the collapsed-thread-actions overlap on desktop).
      // Spacer slack only applies on mobile, where brand and spacer share row width.
      const available = brandLabel.clientWidth + (spacer?.clientWidth || 0);
      // 0.5px tolerance for subpixel rounding.
      setHidden(nonWorkspace + wsMarginPx > available + 0.5);
    };

    const observer = new ResizeObserver(update);
    observer.observe(row);
    observer.observe(brandLabel);
    update();
    return () => observer.disconnect();
  }, [name]);

  return (
    <>
      <span class="connection-status-inline">
        <span class={`status-dot ${status}`} />
      </span>
      {name && (
        <span ref={labelRef} class={`workspace-name-label${hidden ? ' is-hidden' : ''}`}>
          {name}
        </span>
      )}
    </>
  );
}
