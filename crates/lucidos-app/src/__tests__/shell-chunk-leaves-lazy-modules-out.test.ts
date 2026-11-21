/** Source import boundaries preserve initialization and mount gates. Small
 * modules are explicitly grouped into App by Rollup, so this walk no longer
 * claims their code is deferred. smallLazyChunks validates actual emitted
 * membership, library deferral and entry/shell separation in every build.
 * The press probe's cover sample and cold-open UI remain eagerly reachable.
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

describe('source imports preserve lazy initialization gates', () => {
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
