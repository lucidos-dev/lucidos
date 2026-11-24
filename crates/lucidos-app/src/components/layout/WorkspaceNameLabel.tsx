import { useRef, useState, useLayoutEffect } from 'preact/hooks';
import { visibleWorkspaceName } from '../../store/store';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import { getRemPx } from '../../utils/dom';

type LabelFit = 'full' | 'name' | 'none';

export function WorkspaceNameLabel() {
  const name = visibleWorkspaceName.value;
  const labelRef = useRef<HTMLSpanElement>(null);
  const suffixRef = useRef<HTMLSpanElement>(null);
  const [fit, setFit] = useState<LabelFit>('full');

  useLayoutEffect(() => {
    if (!name || !labelRef.current) return;
    const label = labelRef.current;
    const brandLabel = label.closest('.pane-header-brand-center') as HTMLElement | null;
    const brand = brandLabel?.closest('.pane-header-brand') as HTMLElement | null;
    if (!brandLabel || !brand) return;

    const wsMarginPx = 0.0625 * getRemPx();

    const update = () => {
      let nonWorkspace = 0;
      for (const child of Array.from(brandLabel.children) as HTMLElement[]) {
        if (child === label) continue;
        const cs = getComputedStyle(child);
        nonWorkspace += child.scrollWidth + (parseFloat(cs.marginLeft) || 0) + (parseFloat(cs.marginRight) || 0);
      }
      const suffix = suffixRef.current;
      const suffixWidth = suffix
        ? suffix.getBoundingClientRect().width + (parseFloat(getComputedStyle(suffix).marginLeft) || 0)
        : 0;
      const suffixInFlow = suffix !== null && !suffix.classList.contains('is-collapsed');
      const nameWidth = label.scrollWidth - (suffixInFlow ? suffixWidth : 0);
      const room = brandLabel.clientWidth + 0.5 - nonWorkspace - wsMarginPx;
      setFit(nameWidth + suffixWidth <= room ? 'full' : nameWidth <= room ? 'name' : 'none');
    };

    const observer = new ResizeObserver(update);
    observer.observe(brand);
    observer.observe(brandLabel);
    update();
    return () => observer.disconnect();
  }, [name]);

  if (!name) return null;
  return (
    <span ref={labelRef} class={`workspace-name-label${fit === 'none' ? ' is-hidden' : ''}`}>
      {name}
      {REMOTE_GATEWAY && (
        <span
          ref={suffixRef}
          class={`remote-workspace-gateway${fit === 'full' ? '' : ' is-collapsed'}`}
        >@{REMOTE_GATEWAY}</span>
      )}
    </span>
  );
}
