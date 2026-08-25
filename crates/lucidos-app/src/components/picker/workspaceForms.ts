/**
 * Pure logic behind the picker's two footer forms (create / restore).
 *
 * Lives outside `WorkspacePicker.tsx` so the rules that decide what the user is
 * TOLD are unit-testable without a DOM: why Restore is disabled, which existing
 * workspace a name collides with, what address a colliding create will get, and
 * whether a row needs to show its address at all.
 *
 * The one idea all of it serves: a workspace has a free-text **display name**
 * and a derived **address** (the `/slug/` the gateway routes it under, which is
 * also its directory and database name, frozen at create time). Rename edits
 * only the name, so the two diverge, and every message here names the workspace
 * the way the user sees it in the list AND states the address, because a message
 * about an address the UI never shows is unreadable ("personal already exists"
 * when the only workspace on screen is called "personaal").
 */

import { parseWorkspaceNameFromArchive, type WorkspaceStatus } from '../../api/client/control';
import type { Loadable } from '../../store/types';
import { slugifyWorkspaceName, uniqueWorkspaceSlug } from '../../utils/slug';

/** The first run: the list has loaded and there is no workspace yet. Two things
 *  key off it, so it is one predicate: the create form unfolds itself, and it
 *  offers the quick-fill name chips. A LOADING list is not the first run, since
 *  it is empty for a reason that says nothing about what the user has. */
export function isFirstRun(list: Loadable<WorkspaceStatus[]>): boolean {
  return list.status === 'loaded' && list.data.length === 0;
}

/** What the user has filled in on the restore form so far. */
export interface RestoreDraft {
  file: File | null;
  key: string;
  name: string;
}

export const EMPTY_RESTORE_DRAFT: RestoreDraft = { file: null, key: '', name: '' };

/** The one thing standing between the draft and a submittable restore. The
 *  collision case carries the workspace in the way so the UI can offer to delete
 *  that exact workspace. */
export type RestoreBlocker =
  | { kind: 'file' | 'key' | 'name'; message: string }
  | { kind: 'collision'; message: string; existing: WorkspaceStatus };

/** The workspace address a name resolves to, rendered the way the user sees it
 *  in the URL bar. */
export function workspaceAddress(slug: string): string {
  return `/${slug}/`;
}

/** The registered workspace already carrying this display name, or null.
 *  Trimmed and case-insensitive, skipping `exceptId` so renaming a workspace to
 *  what it is already called is not a collision with itself. Mirrors
 *  `Registry::find_by_display_name`, which is the authority.
 *
 *  Two rows reading the same thing cannot be told apart in the picker, so create,
 *  rename and restore all refuse a duplicate. The rule binds writes only:
 *  workspaces that already share a name keep working and show their addresses
 *  (see `showsAddress`) until the user renames one. */
export function nameTakenBy(
  name: string,
  workspaces: readonly WorkspaceStatus[],
  exceptId?: string,
): WorkspaceStatus | null {
  const wanted = name.trim().toLowerCase();
  if (!wanted) return null;
  return workspaces.find((w) => w.id !== exceptId && w.name.trim().toLowerCase() === wanted) ?? null;
}

/** The one sentence all three forms use when a name is taken, matching the
 *  gateway's own refusal. Quotes the existing workspace's name as stored, not
 *  what the user typed: the match ignores case and padding, so "PersonAAA" has
 *  to come back as the "personaaa" they can see in the list. */
export function nameTakenMessage(existing: WorkspaceStatus): string {
  return `You already have a workspace called “${existing.name}”. Choose a different name.`;
}

/** The registered workspace a typed name would collide with, or null. The match
 *  is on the ADDRESS, not the display name: that is what the gateway actually
 *  refuses, and what makes a collision surprising in the first place. */
export function collidingWorkspace(
  name: string,
  workspaces: readonly WorkspaceStatus[],
): WorkspaceStatus | null {
  const trimmed = name.trim();
  if (!trimmed) return null;
  const slug = slugifyWorkspaceName(trimmed);
  return workspaces.find((w) => w.id === slug) ?? null;
}

/** Why Restore is disabled, or null when the draft is ready to submit. Exactly
 *  one reason at a time, in the order the user fills the form in, so the hint
 *  always points at the next thing to do rather than listing everything. */
export function restoreBlocker(
  draft: RestoreDraft,
  workspaces: readonly WorkspaceStatus[],
): RestoreBlocker | null {
  if (!draft.file) {
    return { kind: 'file', message: 'Choose the .enc backup file to restore.' };
  }
  if (!draft.key.trim()) {
    return { kind: 'key', message: 'Enter the backup key you saved when you set up backups.' };
  }
  if (!draft.name.trim()) {
    return { kind: 'name', message: 'Enter a name for the restored workspace.' };
  }
  // The NAME first: it is the collision the user can see, so when both the name
  // and the address it derives are taken, talk about the name.
  const taken = nameTakenBy(draft.name, workspaces);
  if (taken) {
    return { kind: 'collision', existing: taken, message: nameTakenMessage(taken) };
  }
  const existing = collidingWorkspace(draft.name, workspaces);
  if (existing) {
    return {
      kind: 'collision',
      existing,
      message: `The address ${workspaceAddress(existing.id)} is already taken by “${existing.name}”.`,
    };
  }
  return null;
}

/** Non-blocking note when the chosen file doesn't look like a backup archive.
 *  Not an error: the gateway is the authority on whether an archive decrypts,
 *  and a user who renamed their download must still be able to restore it. */
export function restoreFileNote(file: File | null): string | null {
  if (!file) return null;
  if (file.name.toLowerCase().endsWith('.enc')) return null;
  return `“${file.name}” doesn’t look like a .enc backup. Restore will fail if it isn’t one.`;
}

/** Apply a file the user chose or dropped. An EMPTY selection is a no-op, never
 *  a reset: `onChange` fires with no files when the dialog is cancelled, and the
 *  old unconditional assignment silently wiped the already-chosen file and the
 *  name it had filled in, leaving a form that looked complete but couldn't
 *  submit and said nothing about why. A recognizable archive name refills the
 *  name field; an unrecognizable one leaves whatever the user typed. */
export function applyRestoreFile(current: RestoreDraft, file: File | null | undefined): RestoreDraft {
  if (!file) return current;
  const parsed = parseWorkspaceNameFromArchive(file.name);
  return { ...current, file, name: parsed ?? current.name };
}

/** What the create form has to say about the typed name, if anything. Two
 *  different things, in precedence order, which is why they share one function:
 *
 *  - the NAME is taken, which BLOCKS (a second row reading the same thing is
 *    unusable, so the gateway refuses it);
 *  - only the ADDRESS is taken, which does not block: the gateway silently
 *    suffixes (`registry::unique_slug`), and without saying so the workspace
 *    just lands somewhere the name never suggested.
 */
export type CreateNote = { message: string; blocking: boolean } | null;

export function createNote(name: string, workspaces: readonly WorkspaceStatus[]): CreateNote {
  const taken = nameTakenBy(name, workspaces);
  if (taken) return { message: nameTakenMessage(taken), blocking: true };
  const existing = collidingWorkspace(name, workspaces);
  if (!existing) return null;
  const slug = uniqueWorkspaceSlug(
    slugifyWorkspaceName(name.trim()),
    workspaces.map((w) => w.id),
  );
  return {
    message: `“${existing.name}” already uses ${workspaceAddress(existing.id)}, so this one will live at ${workspaceAddress(slug)}.`,
    blocking: false,
  };
}

/** Whether a row should show its address. Only when it would surprise: the
 *  address doesn't match the display name (a rename moved the name off its
 *  address), or two workspaces share a display name and the address is the only
 *  thing telling the rows apart. Silent in the ordinary case. */
export function showsAddress(ws: WorkspaceStatus, all: readonly WorkspaceStatus[]): boolean {
  if (slugifyWorkspaceName(ws.name) !== ws.id) return true;
  return all.some((other) => other.id !== ws.id && other.name === ws.name);
}
