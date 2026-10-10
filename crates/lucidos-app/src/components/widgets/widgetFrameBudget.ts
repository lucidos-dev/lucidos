/** The shared cap on mounted widget frames (ADR 0415).
 *
 *  Each widget frame is a renderer process of its own (ADR 0227). Widget
 *  cards, option widgets and reply embeds mount once they come on screen, and
 *  stay a while after they scroll off. This one budget caps how many are
 *  mounted at once. Past it, the frame furthest from view unmounts first and
 *  draws again when it next comes on screen. Frames in view are never evicted. */

/** How many widget frames may be mounted at once, across every placement. */
export const MAX_MOUNTED_WIDGET_FRAMES = 8;

interface Claim {
  host: HTMLElement;
  evict: () => void;
}

const claims = new Set<Claim>();

/** How far an element sits outside the viewport, in pixels; 0 when on it. */
function distanceFromView(el: HTMLElement): number {
  const rect = el.getBoundingClientRect();
  const height = window.innerHeight;
  if (rect.bottom < 0) return -rect.bottom;
  if (rect.top > height) return rect.top - height;
  return 0;
}

/** Claim a mounted frame for `host`. Past the cap, the frame furthest from
 *  view is evicted: its `evict` runs and its claim ends. A frame in view is
 *  never evicted, since it would draw an empty box nothing re-mounts. Returns
 *  the release for an unmount. */
export function claimWidgetFrame(host: HTMLElement, evict: () => void): () => void {
  const claim: Claim = { host, evict };
  claims.add(claim);
  while (claims.size > MAX_MOUNTED_WIDGET_FRAMES) {
    let furthest: Claim | null = null;
    let furthestDistance = 0;
    for (const c of claims) {
      const distance = distanceFromView(c.host);
      if (distance > furthestDistance) {
        furthest = c;
        furthestDistance = distance;
      }
    }
    if (!furthest) break;
    claims.delete(furthest);
    furthest.evict();
  }
  return () => {
    claims.delete(claim);
  };
}

/** How many frames hold a claim now. */
export function mountedWidgetFrameCount(): number {
  return claims.size;
}
