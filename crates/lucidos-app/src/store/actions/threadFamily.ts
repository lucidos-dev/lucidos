import { threadMap } from '../store';

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
