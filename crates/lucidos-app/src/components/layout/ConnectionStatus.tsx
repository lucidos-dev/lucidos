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
    // The brand shares the row with .pane-header-spacer(s). On MOBILE the brand
    // is absolutely centered (shrink-to-content), so brandLabel.clientWidth has
    // no slack past the text — the trailing spacer holds the row's free width and
    // IS the room the name can occupy, so it must be summed in (dropping it
    // latched the name hidden — the "workspace name gone" bug). On DESKTOP the
    // brand-label is a fixed-width centered box with its own slack and there are
    // no spacer siblings, so the sum is 0.
    const spacers = Array.from(
      row.querySelectorAll(':scope > .pane-header-spacer'),
    ) as HTMLElement[];

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
      // brandLabel.clientWidth is the usable area inside the label; the trailing
      // spacer's width is the extra room the name can grow into on mobile (0 on
      // desktop). A long name is kept from spilling over the leading icons not by
      // this budget but by the bounded .pane-header-brand-label (mobile.css),
      // which ellipsis-truncates .workspace-name-label within the centered box.
      const available =
        brandLabel.clientWidth + spacers.reduce((n, s) => n + s.clientWidth, 0);
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
