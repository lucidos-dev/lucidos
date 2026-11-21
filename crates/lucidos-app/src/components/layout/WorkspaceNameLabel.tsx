import { useRef, useState, useLayoutEffect } from 'preact/hooks';
import { visibleWorkspaceName } from '../../store/store';
import { REMOTE_GATEWAY } from '../../utils/basePath';
import { getRemPx } from '../../utils/dom';

/** How much of the label the brand box can hold: the name with its remote
 *  gateway, the bare name, or nothing. */
type LabelFit = 'full' | 'name' | 'none';

/** The workspace name beside the desktop mark, shown while the pane can hold it.
 *
 *  DESKTOP ONLY. Mobile names the workspace inside the Lucidos menu, on the
 *  Workspaces row's trailing chip, because no phone header row has the width;
 *  desktop shows it in the bar AND keeps that row, so the name is reachable at
 *  any pane width.
 *
 *  A remote workspace adds its gateway (`@cloud`). That suffix is the first
 *  thing to go: when `name @gateway` does not fit, the bare name shows.
 *
 *  Each part is shown WHOLE or not at all, never ellipsised: half a workspace
 *  name is not an identification, and the menu's Workspaces row carries the
 *  full one either way.
 *
 *  The measurement: the host `.pane-header-brand-center` is the flex middle of a
 *  fixed-span cluster (the two thread chevrons take its ends), so its
 *  `clientWidth` is the room actually available to the brand. The label stays
 *  in the DOM while hidden so `scrollWidth` keeps reporting the name's natural
 *  width. A collapsed suffix is taken out of flow but still laid out, so its
 *  own box keeps reporting its width. Both decide when a part comes back. */
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

    // Mirrors .workspace-name-label margin-left in panels/shell.css. is-hidden
    // zeros it (so hiding frees the gap), so the live computed margin cannot be
    // read back once hidden: the constant keeps the threshold invariant.
    const wsMarginPx = 0.0625 * getRemPx();

    const update = () => {
      // Everything else in the box, which is the mark's slot.
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
      // 0.5px tolerance for subpixel rounding.
      const room = brandLabel.clientWidth + 0.5 - nonWorkspace - wsMarginPx;
      setFit(nameWidth + suffixWidth <= room ? 'full' : nameWidth <= room ? 'name' : 'none');
    };

    // The box tracks the pane, so watch the brand region it is centred in as
    // well as the box itself: the box's own width is derived from the region's.
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
