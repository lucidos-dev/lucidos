/**
 * Side-effecting entry that wires per-workspace localStorage namespacing to the
 * real globals. Imported on the FIRST line of `main.tsx` so it runs before any
 * module-init `localStorage` read (notably `store/store.ts`, which reads at
 * module-load): ESM evaluates imports depth-first in source order, so this
 * module — and its only dependency, `basePath.ts` — fully evaluate before `App`
 * and the store are imported.
 *
 * Kept separate from `workspaceStorage.ts` (which is pure + dependency-injected)
 * so unit tests can import the logic without triggering the global install.
 */
import { WORKSPACE_ID } from './basePath';
import { installWorkspaceStorage } from './workspaceStorage';

if (typeof localStorage !== 'undefined') {
  installWorkspaceStorage(localStorage, WORKSPACE_ID);
}
