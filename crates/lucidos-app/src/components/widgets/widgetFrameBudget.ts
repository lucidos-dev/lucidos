
export const MAX_MOUNTED_WIDGET_FRAMES = 8;

interface Claim {
  host: HTMLElement;
  evict: () => void;
}

const claims = new Set<Claim>();

function distanceFromView(el: HTMLElement): number {
  const rect = el.getBoundingClientRect();
  const height = window.innerHeight;
  if (rect.bottom < 0) return -rect.bottom;
  if (rect.top > height) return rect.top - height;
  return 0;
}

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

export function mountedWidgetFrameCount(): number {
  return claims.size;
}
