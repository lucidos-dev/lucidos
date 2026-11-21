import { threadMap } from '../store';

/** Walk parentThreadId from every thread in the map to collect the target +
 *  every transitive descendant. Mirrors the backend cascade scope, which
 *  archive and delete share, so an optimistic flip covers the whole family in
 *  one stroke.
 *
 *  Delete needs the same set for a harsher reason: archive flips a column on
 *  each member and delete removes the rows. The thread menu walks it to name
 *  the sub-thread that blocks. Its own module, so that derivation need not
 *  import `threads.ts`. */
export function collectThreadFamily(rootId: string): Set<string> {
  const childrenByParent = new Map<string, string[]>();
  for (const t of threadMap.value.values()) {
    const p = t.meta.parentThreadId;
    if (!p) continue;
    const bucket = childrenByParent.get(p);
    if (bucket) bucket.push(t.meta.id); else childrenByParent.set(p, [t.meta.id]);
  }
  const seen = new Set<string>();
  const stack: string[] = [rootId];
  while (stack.length > 0) {
    const id = stack.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    const kids = childrenByParent.get(id);
    if (kids) stack.push(...kids);
  }
  return seen;
}
