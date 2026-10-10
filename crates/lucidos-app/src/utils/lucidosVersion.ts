/** The one number that names LUCIDOS ITSELF: the umbrella release from the
 *  repo's `RELEASE` file, which the engine reports on `/health` and System >
 *  Overview labels "Lucidos".
 *
 *  It is deliberately the only version the Lucidos menu's identity row shows.
 *  The other three on that page each identify a PART (the engine's
 *  CalVer, this client's build, the service worker's build), and which of them a
 *  client can honestly name changes with the platform: the row used to read
 *  "dev" on a Vite dev server, a hex build id on a built web client, and the
 *  shell's app version inside Tauri. Three answers to "what am I running?" is no
 *  answer. The release is the same one everywhere, which is what makes it the
 *  right thing for the place the product names itself.
 *
 *  Null release means the engine has not answered `/health` yet, or answered
 *  without one. The row then names its DESTINATION rather than blanking its
 *  pill, so it keeps its shape and still says where versions live.
 */
export function lucidosVersionLabel(release: string | null, dirty: boolean): string {
  if (!release) return 'System';
  return dirty ? `${release} *` : release;
}

/** The identity row's accessible name and hover tooltip, or undefined when the
 *  visible label already says everything.
 *
 *  `dirty` is the engine's `release_dirty`: HEAD has moved past the commit that
 *  bumped RELEASE, so the running code sits somewhere after the published
 *  snapshot. The label marks that with a trailing `*`, the same marker the
 *  System page uses, and a bare asterisk in a pill names nothing on its own,
 *  which is why the System page spells it out in a footnote and this spells it
 *  out here.
 *
 *  Undefined with no release, rather than a sentence invented for a state that
 *  lasts until `/health` answers. An `aria-label` REPLACES a control's visible
 *  text as its accessible name, so copy that does not contain the word on screen
 *  ("System") would leave the row unaddressable by voice while saying nothing
 *  the row does not already say. */
export function lucidosVersionTooltip(release: string | null, dirty: boolean): string | undefined {
  if (!release) return undefined;
  return dirty
    ? `Lucidos ${release} · code has changed since this release`
    : `Lucidos ${release}`;
}
