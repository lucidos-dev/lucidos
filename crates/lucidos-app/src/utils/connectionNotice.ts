/**
 * The words for the connection state: what the mark's accessible name says, what
 * the desktop tooltip says, what the Lucidos menu's notice leads with, what the
 * header's connection bar states, and what Settings > System spells under its
 * status row.
 *
 * A leaf module because the number of surfaces outgrew the one that used to own
 * it. This lived in `components/layout/HeaderMark.tsx` while the mark and its
 * menu were the only readers; a bar and a settings row importing from there
 * would be importing a menu to get a sentence, and the first thing anyone does
 * with a sentence that is awkward to import is retype it.
 *
 * Everything here is pure and has no view of its own, so `vnodeToText`-style
 * tests can flatten the surfaces that render it and the table itself is unit
 * tested directly: three states times two name cases is exactly the sort of
 * string table that rots silently.
 *
 * The GATEWAY's reading of a workspace is a different claim and lives in
 * `utils/workspaceState.ts`. That one answers "what does the gateway know about
 * this workspace", carrying its `last_error`; this one answers "can this client
 * reach THIS workspace's engine", decided solely by the `/api/v1/health` poll in
 * `store/actions/connection.ts`. Merging them would force one of the two to say
 * something it cannot know.
 */

import type { ConnectionStatus } from '../store/types';

/** Readable spelling of the connection light, naming what it is connected TO.
 *
 *  The mark carries the state as colour and motion, and this is its readable
 *  half, read by three surfaces: the toggle's accessible name, the desktop hover
 *  tooltip, and the sentence the menu's own notice leads with (see
 *  `connectionNotice`, which sentence-cases exactly this). One table, so what a
 *  screen reader is told and what the panel says cannot drift into two
 *  different claims about one state.
 *
 *  On the tooltip the workspace matters as much as the state: the name beside
 *  the mark hides itself when the pane is narrow, so this is what answers
 *  "connected to what?" at any width.
 *
 *  Each state brings its own preposition rather than sharing one, because
 *  "disconnected to dev" is not English. With no workspace name yet (before
 *  /health answers) the phrase is the bare state word. */
const CONNECTION_PHRASE: Record<string, (ws: string) => string> = {
  connected: (ws) => `connected to ${ws}`,
  connecting: (ws) => `connecting to ${ws}`,
  disconnected: (ws) => `disconnected from ${ws}`,
};

export function connectionPhrase(status: string, workspace: string | null): string {
  const phrase = CONNECTION_PHRASE[status];
  if (!phrase) return status;
  return workspace ? phrase(workspace) : status;
}

/** What the degraded surfaces say, and nothing at all while the mark is lit.
 *
 *  A state with no line here renders no notice anywhere, and `connected` is the
 *  only one the closed `ConnectionStatus` union leaves out. That is the whole
 *  condition, and it is deliberately the same one the MARK recedes on:
 *  `styles/header-mark.css` dims disconnected and breathes connecting, so a
 *  dimmed glyph above a panel that mentions nothing is what this exists to end.
 *
 *  In the menu the condition is the STATE, not the host that opened the panel,
 *  and on one host those differ. The mobile threads row's mark carries no
 *  `data-conn` at all (see `BrandMenuButton`: a glyph dimming itself among a row
 *  of icons that do not reads as disabled), so there is no light on that pane to
 *  explain, and the notice is instead the only place the state appears. Keying
 *  it on the host to "match" the mark would take it away exactly where it is
 *  worth most.
 *
 *  The disconnected line names no remedy, on purpose. Restart posts to the
 *  engine we cannot reach, and Refresh reloads a client that is not the thing
 *  that broke, so pointing at either would be wrong in the ordinary case. The 5s
 *  health poll in `store/actions/connection.ts` genuinely does recover on its
 *  own, so that is what it promises instead. */
const CONNECTION_DETAIL: Record<string, string> = {
  connecting: 'Waiting for the workspace to answer.',
  // Scoped to THIS WORKSPACE rather than the app, because a bare "nothing loads
  // or sends" is refuted by the row directly under it in the menu:
  // `connectionStatus` is driven solely by `/api/v1/health` against this
  // workspace's engine, while the Workspaces row talks to the GATEWAY
  // (`/~/api/v1/control/*`, a different process), so unfolding the list and
  // switching away still work while this engine is unreachable. The narrower
  // claim is both true and more useful, since switching is the one thing in the
  // panel that still goes anywhere.
  // Two lines in the panel's 17.5rem, and that is the ceiling worth spending:
  // the notice pushes every row below it down, so a third line buys nothing the
  // first two have not already said.
  disconnected: 'Nothing in this workspace loads or sends. Still trying.',
};

export function connectionNotice(
  status: ConnectionStatus,
  workspace: string | null,
): { title: string; detail: string } | null {
  const detail = CONNECTION_DETAIL[status];
  if (!detail) return null;
  // Sentence-cased rather than a second string table: the preposition each
  // state wants is already decided once, above.
  const phrase = connectionPhrase(status, workspace);
  return { title: phrase.charAt(0).toUpperCase() + phrase.slice(1), detail };
}

/** The whole notice as one sentence, for a surface that has an accessible name
 *  but no room to render both halves apart (the bar's `aria-label`, a tooltip).
 *  Derived rather than authored, so it cannot drift from what is on screen. */
export function connectionNoticeSentence(
  status: ConnectionStatus,
  workspace: string | null,
): string | null {
  const notice = connectionNotice(status, workspace);
  return notice ? `${notice.title}. ${notice.detail}` : null;
}
