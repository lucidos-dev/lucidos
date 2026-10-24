/** The transcript's skeleton overlay has to SURVIVE the cold-open switch.
 *
 *  `ThreadView` returns two different trees. Before `loadAllThreads` lands, the
 *  thread is absent from the map and the early return draws a bare wrap. After
 *  it, the main return leads with a header. An unkeyed diff matches those
 *  children by index, finds a header where the wrap was, and rebuilds the
 *  subtree under it.
 *
 *  A remounted element cannot run a CSS transition, so the overlay's crossfade
 *  becomes a snap. Only a cold open crosses the two trees, which is why the
 *  report was iOS-only: that PWA is evicted constantly, so nearly every open is
 *  cold, and on desktop nearly none are.
 *
 *  A source scan, because this suite has no DOM and node identity is exactly
 *  what a vnode cannot show. Same idiom as `skeleton-guard.test.ts`. */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = readFileSync(resolve(here, '../ThreadView.tsx'), 'utf8');

/** Every opening tag for `name`, whether an element or a component. */
function openingTags(name: string): string[] {
  const pattern = new RegExp(`<${name}(?=[\\s/>])[^>]*>`, 'g');
  return SOURCE.match(pattern) ?? [];
}

describe("ThreadView's two trees keep one skeleton overlay", () => {
  it('renders the overlay in exactly the two returns this is about', () => {
    // A third call site would need its own key, and would not be covered by
    // the assertions below simply because they count.
    expect(openingTags('ThreadSkeletonOverlay')).toHaveLength(2);
  });

  it('keys every overlay, so the diff matches it across the switch', () => {
    for (const tag of openingTags('ThreadSkeletonOverlay')) {
      expect(tag, tag).toContain('key="skeleton"');
    }
  });

  it('keys the wrap the overlay lives in, which the header displaces', () => {
    const wraps = openingTags('div').filter((t) => t.includes('class="thread-content-wrap'));
    expect(wraps).toHaveLength(2);
    for (const tag of wraps) {
      expect(tag, tag).toContain('key="wrap"');
    }
  });

  it('keys the scroll container beside it, so neither is matched by index', () => {
    const contents = openingTags('div').filter((t) => t.includes('class="thread-content visible"'));
    expect(contents).toHaveLength(2);
    for (const tag of contents) {
      expect(tag, tag).toContain('key="content"');
    }
  });
});
