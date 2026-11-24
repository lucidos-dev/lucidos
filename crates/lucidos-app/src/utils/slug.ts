export function slugifyWorkspaceName(name: string): string {
  let out = '';
  let prevDash = false;
  for (const ch of name) {
    if (/[a-z0-9]/i.test(ch)) {
      out += ch.toLowerCase();
      prevDash = false;
    } else if (!prevDash && out.length > 0) {
      out += '-';
      prevDash = true;
    }
  }
  out = out.replace(/-+$/, '');
  return out || 'workspace';
}

export function uniqueWorkspaceSlug(base: string, taken: readonly string[]): string {
  if (!taken.includes(base)) return base;
  for (let n = 2; ; n++) {
    const candidate = `${base}-${n}`;
    if (!taken.includes(candidate)) return candidate;
  }
}
