/**
 * Modules the shell chunk must reach only through a dynamic `import()`. Each
 * one spent first-paint budget (ADR 0436) on something the first frame never
 * draws, so a static import that pulls it back fails here first.
 *
 * - The HTML preview's frame bridge carries two raw iframe scripts. "Open in
 *   new tab" takes its URL helper from `store/actions/frame-capability.ts`.
 * - The composer's press probe is a diagnostic. `startShell` loads it on its
 *   own, keeping only its cover sample (`pressCover.ts`), which must install
 *   before any overlay's listener.
 * - Menus and dialogs that open on demand.
 *
 * What a cold open can draw at once stays eager (ADR 0288). The progress
 * dialog stays too: it is the engine restart's surface, so no chunk fetch may
 * stand in its way.
 *
 * This walks the static import graph from `App.tsx`, as Rollup does for the
 * shell chunk (`staticImportGraph.ts`).
 */
import { describe, it, expect } from 'vitest';
import { chainTo, srcPath, staticGraph } from './staticImportGraph';

const LAZY = [
  'components/files/previewFrameBridge.ts',
  'components/chat/deadPressProbe.ts',
  'components/layout/BrandMenuRows.tsx',
  'components/layout/ActivityMenuRows.tsx',
  'components/layout/NotificationsMenuRows.tsx',
  'components/layout/WorkspaceMenuRows.tsx',
  'components/shared/ThreadOverflowItems.tsx',
  'store/actions/threads-delete.ts',
  'components/chat/controlMenuPanelParts.ts',
  'components/shared/ModelSelectionPicker.tsx',
  'components/shared/ControlOptionList.tsx',
  'components/chat/FrontendPreviewSection.tsx',
  'components/layout/Drawer.tsx',
  'components/shared/ConfirmDialog.tsx',
  'components/shared/PromptDialog.tsx',
  'store/actions/find-targets.ts',
  'store/actions/transcript-find.ts',
  'components/chat/CodingAgentControlMenu.tsx',
  'components/chat/RungControl.tsx',
  'components/chat/LucidosControlMenu.tsx',
  'components/chat/MicrophonePicker.tsx',
  'components/widgets/WidgetMenu.tsx',
];

const EAGER = [
  'components/chat/PermissionCard.tsx',
  'components/chat/QuestionCard.tsx',
  'components/layout/ThreadFilterPanel.tsx',
  'components/shared/ProgressDialog.tsx',
];

describe('the shell chunk leaves its lazy modules out', () => {
  const graph = staticGraph(srcPath('App.tsx'));

  it('reaches shell startup and the header actions, so the walk itself works', () => {
    expect(graph.has(srcPath('shellStartup.ts'))).toBe(true);
    expect(graph.has(srcPath('components/layout/ContentHeaderActions.tsx'))).toBe(true);
  });

  it('keeps the press probe\'s cover sample in the shell, so it installs before any overlay', () => {
    expect(graph.has(srcPath('components/chat/pressCover.ts'))).toBe(true);
  });

  for (const eager of EAGER) {
    it(`keeps ${eager}, which a cold open can draw at once`, () => {
      expect(graph.has(srcPath(eager))).toBe(true);
    });
  }

  for (const lazy of LAZY) {
    it(`never reaches ${lazy} by a static import`, () => {
      const file = srcPath(lazy);
      expect(graph.has(file), graph.has(file) ? chainTo(graph, file) : '').toBe(false);
    });
  }
});
