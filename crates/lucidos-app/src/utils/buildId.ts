/** Whether a build id is the un-stamped `__LUCIDOS_BUILD_ID__` placeholder that
 *  the `virtual:build-id` plugin emits when the `lucidos-sw-stamp` plugin is
 *  inert (the live dev server — see `vite.config.ts`). Such an id carries NO
 *  staleness signal: it is the same string in every build, so comparing two of
 *  them can never detect a newer client.
 *
 *  Every consumer has to screen for it — the refresh fast path and the badge /
 *  toast sync bail out, and the System page's build-id rows show "dev" instead —
 *  so the placeholder's shape is written down here once. Lives in `utils/`
 *  rather than beside its callers in `hooks/sw-update.ts` deliberately: it is a
 *  pure string test with no DOM, service-worker, or store dependencies, and
 *  suites that replace the whole `sw-update` module (`client-update.test.ts`)
 *  must not have to re-stub it to keep working. */
export function isUnstampedBuildId(id: string): boolean {
  return id.startsWith('__');
}
