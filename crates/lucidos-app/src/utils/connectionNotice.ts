/**
 * The words for the connection state: what the mark's accessible name says, what
 * the desktop tooltip says, what the Lucidos menu's notice leads with, what the
 * header's connection bar states, and what System > Overview spells under its
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
 *
 * A failed poll proves one thing: this client got no answer. It cannot tell an
 * engine that stopped from a client that cannot reach a healthy one. So every
 * sentence here claims the reach, and nothing here blames the engine. A
 * packaged desktop window once sat unreachable while a phone on the same
 * gateway kept working.
 *
 * That boundary is also why nothing below separates "the gateway answers, the
 * engine poll does not" from "nothing answers". Telling those apart needs the
 * gateway's reading, which this module has no business holding. So it takes the
 * weaker claim one failed poll does support.
 */

import type { ConnectionStatus } from '../store/types';

/** What the client is reaching for, named as what it is: the ENGINE serving
 *  this workspace, not the workspace itself.
 *
 *  A workspace is a place the user keeps things, and it is still there. Its
 *  engine is the process this client reaches for, so the engine is what the
 *  sentence names. "Cannot reach dev" reads as having lost the workspace, which
 *  is alarming and untrue: the gateway keeps listing and switching workspaces
 *  throughout.
 *
 *  Before `/health` first answers there is no name, so the target is the bare
 *  noun. A title of one bare state word would leave the sentence under it with
 *  nothing to be about. */
function connectionTarget(workspace: string | null): string {
  return workspace ? `the ${workspace} engine` : 'the engine';
}

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
 *  Each state brings its own verb rather than sharing one, and the degraded one
 *  is deliberately weak. "Disconnected from" reads as a link the engine dropped,
 *  which one failed poll cannot support. "Cannot reach" says the part it does
 *  support: nothing got through from here. What the three share is the target,
 *  composed once in `connectionTarget`. */
const CONNECTION_PHRASE: Record<string, (target: string) => string> = {
  connected: (target) => `connected to ${target}`,
  connecting: (target) => `connecting to ${target}`,
  disconnected: (target) => `cannot reach ${target}`,
};

export function connectionPhrase(status: string, workspace: string | null): string {
  const phrase = CONNECTION_PHRASE[status];
  if (!phrase) return status;
  return phrase(connectionTarget(workspace));
}

/** How much of the detail a surface asks for.
 *
 *  `full` is the explainer, and one surface earns it: the connection bar, which
 *  spans the window and has a line to spend. `short` is for a surface the reader
 *  has to reach for, the menu notice and the hover tooltip. There the explainer
 *  wrapped to three lines and pushed the panel's rows down, to say what the
 *  title had mostly said already.
 *
 *  A caller names it rather than taking a default, so a new surface has to
 *  decide which one it is. */
export type NoticeLength = 'short' | 'full';

/** What the degraded surfaces say, and nothing at all while the mark is lit.
 *
 *  Two clauses per state, because two surfaces want different amounts of it.
 *  One sentence cannot be cut at the callsite without becoming a second copy.
 *  The CONSEQUENCE is what stopped working, the RECOVERY is what happens next.
 *  Short is the recovery alone: the title already names the state, so what a
 *  reader still wants there is whether it fixes itself.
 *
 *  A state with no entry here renders no notice anywhere, and `connected` is the
 *  only one the closed `ConnectionStatus` union leaves out. That is the whole
 *  condition, and it is deliberately the same one the MARK recedes on:
 *  `styles/header-mark.css` dims disconnected and breathes connecting, so a
 *  dimmed glyph above a panel that mentions nothing is what this exists to end.
 *
 *  In the menu the condition is the STATE, not the host that opened the panel,
 *  and on one host those differ. The mobile threads row's mark carries no
 *  `data-conn` at all, so that pane has no light to explain. The notice is
 *  instead the only place the state appears there. Keying it on the host would
 *  take it away exactly where it is worth most. */
const CONNECTION_DETAIL: Record<string, { consequence: string; recovery: string }> = {
  connecting: {
    consequence: 'Threads and messages will not load or send in this window yet.',
    recovery: 'Waiting for an answer.',
  },
  // The consequence scopes on two axes, because a refutation waits on each. It
  // names the engine's own content. The Workspaces row in the menu reaches the
  // GATEWAY, keeps listing and switching, and does it from inside this window.
  // It also names this window, because another client on the same workspace can
  // load and send fine. A phone did exactly that through the outage this
  // wording came from.
  //
  // The recovery names no remedy, on purpose. Restart posts to an engine we
  // cannot reach, and Refresh reloads a client that is not what broke. The 5s
  // health poll in `store/actions/connection.ts` does recover on its own, so
  // the line promises that instead, at the interval it runs at.
  disconnected: {
    consequence: 'Threads and messages will not load or send in this window.',
    recovery: 'Retrying every few seconds.',
  },
};

/** Whether this state has anything to say, for a surface deciding whether to
 *  exist at all. Keyed on the same table the words come from, so a bar and a
 *  notice cannot disagree about which states are worth mentioning. */
export function hasConnectionNotice(status: string): boolean {
  return CONNECTION_DETAIL[status] !== undefined;
}

export function connectionNotice(
  status: ConnectionStatus,
  workspace: string | null,
  length: NoticeLength,
): { title: string; detail: string } | null {
  const detail = CONNECTION_DETAIL[status];
  if (!detail) return null;
  // Sentence-cased rather than a second string table: the preposition each
  // state wants is already decided once, above.
  const phrase = connectionPhrase(status, workspace);
  return {
    title: phrase.charAt(0).toUpperCase() + phrase.slice(1),
    detail: length === 'full' ? `${detail.consequence} ${detail.recovery}` : detail.recovery,
  };
}

/** The whole notice as one sentence, for a surface that has an accessible name
 *  but no room to render both halves apart (the bar's `aria-label`, a tooltip).
 *  Derived rather than authored, so it cannot drift from what is on screen. */
export function connectionNoticeSentence(
  status: ConnectionStatus,
  workspace: string | null,
  length: NoticeLength,
): string | null {
  const notice = connectionNotice(status, workspace, length);
  return notice ? `${notice.title}. ${notice.detail}` : null;
}
