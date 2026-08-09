import { useRef, useState, useLayoutEffect } from 'preact/hooks';
import { visibleWorkspaceName } from '../../store/store';
import { getRemPx } from '../../utils/dom';

/** The workspace name beside the desktop mark, shown while the pane can hold it.
 *
 *  DESKTOP ONLY. Mobile names the workspace inside the Lucidos menu, on the
 *  Workspaces row's trailing chip, because no phone header row has the width;
 *  desktop shows it in the bar AND keeps that row, so the name is reachable at
 *  any pane width.
 *
 *  It used to render the connection dot as well, which is what it was named for
 *  (`ConnectionStatus`). The mark took that job over (`data-conn` in
 *  styles/header-mark.css), and one light per row means the dot is gone: this is
 *  the name and its fit measurement, nothing else.
 *
 *  Shown WHOLE or not at all, never ellipsised: half a workspace name is not an
 *  identification, and the menu's Workspaces row carries the full one either
 *  way.
 *
 *  The measurement: the host `.pane-header-brand-center` is the flex middle of a
 *  fixed-span cluster (the two thread chevrons take its ends), so its
 *  `clientWidth` is the room actually available to the brand. When the mark plus
 *  this name's leading gap plus the name itself do not fit in it, the name
 *  hides. It stays in the DOM while hidden so `scrollWidth` keeps reporting its
 *  natural width, which is what decides when to bring it back. */
export function WorkspaceNameLabel() {
  const name = visibleWorkspaceName.value;
  const labelRef = useRef<HTMLSpanElement>(null);
  const [hidden, setHidden] = useState(false);

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
      // The name is shown WHOLE or not at all: it never ellipsises. Half a
      // workspace name is not an identification, and the menu's Workspaces row
      // has the full one either way, so the honest states are "here it is" and
      // "no room". `scrollWidth` is the natural width, which the element keeps
      // reporting while hidden, so the measurement can bring it back.
      // 0.5px tolerance for subpixel rounding.
      setHidden(nonWorkspace + wsMarginPx + label.scrollWidth > brandLabel.clientWidth + 0.5);
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
    <span ref={labelRef} class={`workspace-name-label${hidden ? ' is-hidden' : ''}`}>
      {name}
    </span>
  );
}
