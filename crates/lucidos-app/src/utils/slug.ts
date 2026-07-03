/** Slug a workspace name the way the gateway registry does (lowercase,
 *  non-alphanumerics → single `-`, trimmed). Mirrors `registry::slugify` in
 *  `crates/lucidos-gateway/src/registry.rs`, so the frontend can predict the
 *  `/<slug>/` path the gateway serves a workspace under — used by the picker
 *  (collision prediction), cross-workspace navigation, and thread-link hrefs.
 *
 *  Leaf util (no app/store/api imports) so any layer can depend on it. */
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
