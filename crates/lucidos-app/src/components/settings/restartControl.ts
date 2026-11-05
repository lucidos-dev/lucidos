/** Which Settings surface owns the engine-restart control.
 *
 *  The control has two homes, and which one it takes is decided by a single
 *  fact: whether the engine is a PACKAGED install (the `packaged` flag from
 *  /health, i.e. the macOS DMG app or the headless tarball).
 *
 *  - dev (`'overview'`) → System > Overview > Maintenance, labelled
 *    "Rebuild & Restart", because there the restart really does rebuild: it
 *    POSTs /restart, which re-runs `web-dev.sh --engine-only`.
 *  - packaged (`'debugging'`) → System > Debugging, labelled "Restart Engine".
 *    A packaged install ships its binary and has no source, so its restart only
 *    respawns the launchd service. "Rebuild" was a lie there, and a pure
 *    recovery action does not belong on the page a user opens to read their
 *    version, so it sits with the other diagnostics instead.
 *
 *  Returning the home (rather than handing each site its own boolean) is what
 *  makes "both render" and "neither renders" unrepresentable: each call site
 *  compares this against its OWN name, so the two branches are exhaustive and
 *  mutually exclusive by construction. */
export type RestartControlHome = 'overview' | 'debugging';

export function restartControlHome(packaged: boolean): RestartControlHome {
  return packaged ? 'debugging' : 'overview';
}
