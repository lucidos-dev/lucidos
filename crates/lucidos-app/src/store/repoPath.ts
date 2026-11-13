// The `repo:` locator codec. A leaf module with no imports, so a pure util
// (`utils/linkifyPaths`) can parse a locator without loading the store.

/** A file in a registered repository clone, named by an encoded `repo:` string.
 *
 *  A union rather than one shape with two optional fields, because the two
 *  modes need DIFFERENT qualifiers. A `file` names a git revision, a `diff`
 *  names the Change whose hunks to show, and neither is meaningful in the
 *  other's mode. */
export type RepoLocator =
  | {
      repoId: string;
      mode: 'file';
      /** The git revision to read the file at (a branch, tag or sha), or
       *  undefined for the clone's current `HEAD`. */
      ref?: string;
      path: string;
    }
  | { repoId: string; mode: 'diff'; changeId?: string; path: string };

/** Encode a repo file locator for the panel overlay.
 *
 *  The qualifier is embedded in the mode segment (`file#<ref>`,
 *  `diff#<changeId>`) rather than added as a fourth colon-separated field. It
 *  therefore survives nav history persistence AND stays unambiguous: a git ref
 *  cannot contain a colon (`git check-ref-format`) and a path can, so the
 *  "everything after the third colon is the path" rule still holds. Without
 *  the embedded changeId, reloading on a diff view spins forever, because
 *  repoDiff is runtime-only state.
 *
 *  Takes the parsed shape so this is the exact inverse of `parseRepoPath`:
 *  `encodeRepoPath(parseRepoPath(s)) === s` for every locator `s` that
 *  parses. */
export function encodeRepoPath(locator: RepoLocator): string {
  const qualifier = locator.mode === 'file' ? locator.ref : locator.changeId;
  const modeSeg = qualifier ? `${locator.mode}#${qualifier}` : locator.mode;
  return `repo:${locator.repoId}:${modeSeg}:${locator.path}`;
}

/** Decode a repo file path from the panel overlay, or null when it is not one.
 *
 *  Four forms, all of them live:
 *
 *    repo:<repoId>:file:<path>              the clone's current HEAD
 *    repo:<repoId>:file#<ref>:<path>        that branch, tag or sha
 *    repo:<repoId>:diff#<changeId>:<path>   a Change's diff
 *    repo:<repoId>:diff:<path>              legacy, changeId-less, still parses
 *
 *  Every segment must be non-empty. This is the single predicate deciding "is
 *  this a repo path" for `normalizeDataPath`, `ContentPane`'s routing and
 *  `openEncodedRepoFilePreview`, and `file_path` reaches it from OUTSIDE the
 *  app. A structurally incomplete encoding like `repo::file:x` would otherwise
 *  parse into an empty repoId, path or ref. That opens a preview which can
 *  only 404, instead of falling back to the data-path preview.
 *
 *  The qualifier is sliced at the FIRST `#`, so a ref containing one survives
 *  intact (`#` is legal in a ref name, unlike `:`). A GitHub-style `#L510`
 *  line suffix still works, because `extractRepoFileTargetFromHref` strips it
 *  before the locator reaches here. */
export function parseRepoPath(encoded: string): RepoLocator | null {
  if (!encoded.startsWith('repo:')) return null;
  const [, repoId, modeSeg, ...rest] = encoded.split(':');
  const path = rest.join(':');
  if (!repoId || !path || !modeSeg) return null;

  const hash = modeSeg.indexOf('#');
  const mode = hash === -1 ? modeSeg : modeSeg.slice(0, hash);
  const qualifier = hash === -1 ? undefined : modeSeg.slice(hash + 1);
  // Present but empty (`file#:x`, `diff#:x`) is malformed, not "unqualified".
  if (qualifier === '') return null;

  if (mode === 'file') return { repoId, mode, ref: qualifier, path };
  if (mode === 'diff') return { repoId, mode, changeId: qualifier, path };
  return null;
}
