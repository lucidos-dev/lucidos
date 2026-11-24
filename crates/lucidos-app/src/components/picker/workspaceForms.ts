
import { ARCHIVE_SUFFIXES, parseWorkspaceNameFromArchive, type WorkspaceStatus } from '../../api/client/control';
import type { Loadable } from '../../store/types';
import { formatTimeAgo } from '../../utils/formatTime';
import { slugifyWorkspaceName, uniqueWorkspaceSlug } from '../../utils/slug';

export function isFirstRun(list: Loadable<WorkspaceStatus[]>): boolean {
  return list.status === 'loaded' && list.data.length === 0;
}

export interface RestoreDraft {
  file: File | null;
  key: string;
  name: string;
}

export const EMPTY_RESTORE_DRAFT: RestoreDraft = { file: null, key: '', name: '' };

export type RestoreBlocker =
  | { kind: 'file' | 'key' | 'name'; message: string }
  | { kind: 'collision'; message: string; existing: WorkspaceStatus };

export function workspaceAddress(slug: string): string {
  return `/${slug}/`;
}

export function nameTakenBy(
  name: string,
  workspaces: readonly WorkspaceStatus[],
  exceptId?: string,
): WorkspaceStatus | null {
  const wanted = name.trim().toLowerCase();
  if (!wanted) return null;
  return workspaces.find((w) => w.id !== exceptId && w.name.trim().toLowerCase() === wanted) ?? null;
}

export function nameTakenMessage(existing: WorkspaceStatus): string {
  return `You already have a workspace called “${existing.name}”. Choose a different name.`;
}

export function collidingWorkspace(
  name: string,
  workspaces: readonly WorkspaceStatus[],
): WorkspaceStatus | null {
  const trimmed = name.trim();
  if (!trimmed) return null;
  const slug = slugifyWorkspaceName(trimmed);
  return workspaces.find((w) => w.id === slug) ?? null;
}

export function restoreBlocker(
  draft: RestoreDraft,
  workspaces: readonly WorkspaceStatus[],
): RestoreBlocker | null {
  if (!draft.file) {
    return { kind: 'file', message: 'Choose the .age or .enc backup file to restore.' };
  }
  if (!draft.key.trim()) {
    return { kind: 'key', message: 'Enter the backup key you saved when you set up backups.' };
  }
  if (!draft.name.trim()) {
    return { kind: 'name', message: 'Enter a name for the restored workspace.' };
  }
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

export function restoreFileNote(file: File | null): string | null {
  if (!file) return null;
  if (ARCHIVE_SUFFIXES.some((suffix) => file.name.toLowerCase().endsWith(suffix))) return null;
  return `“${file.name}” doesn’t look like a backup archive. Restore will fail if it isn’t one.`;
}

export function applyRestoreFile(current: RestoreDraft, file: File | null | undefined): RestoreDraft {
  if (!file) return current;
  const parsed = parseWorkspaceNameFromArchive(file.name);
  return { ...current, file, name: parsed ?? current.name };
}

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

export function showsAddress(ws: WorkspaceStatus, all: readonly WorkspaceStatus[]): boolean {
  if (slugifyWorkspaceName(ws.name) !== ws.id) return true;
  return all.some((other) => other.id !== ws.id && other.name === ws.name);
}

export type BackupNote = { text: string; level: 'ok' | 'warn' } | null;

export function backupNote(ws: WorkspaceStatus): BackupNote {
  const backup = ws.last_successful_backup;
  if (!backup) return null;
  if (backup.at) {
    const when = formatTimeAgo(new Date(backup.at));
    return { text: `Backed up ${when}`, level: backup.stale ? 'warn' : 'ok' };
  }
  return {
    text: backup.configured ? 'Never backed up' : 'Not backed up',
    level: 'warn',
  };
}
