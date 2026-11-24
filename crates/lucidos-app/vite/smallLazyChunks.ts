import type { Plugin } from 'vite';
import { bundleChunks, findShellChunk, type BundleChunk } from './shellChunk';
import { firstPaintChunks } from './chunkBudget';

const SHELL_MODULES = [
  '/packages/lucidos-sdk/src/find.ts',
  '/src/App.tsx',
  '/src/api/client/chatgptPlan.ts',
  '/src/api/client/claudeGateway.ts',
  '/src/api/client/frontendPreview.ts',
  '/src/api/client/gatewayUsersCompat.ts',
  '/src/api/client/landingLane.ts',
  '/src/components/apps/AppCard.tsx',
  '/src/components/apps/AppSearchBar.tsx',
  '/src/components/apps/AppUiEditModal.tsx',
  '/src/components/apps/AppUiInline.tsx',
  '/src/components/apps/AppsView.tsx',
  '/src/components/apps/NewAppModal.tsx',
  '/src/components/apps/ReusableWidgetsSection.tsx',
  '/src/components/changes/ChangesView.tsx',
  '/src/components/chat/CheckpointDiffModal.tsx',
  '/src/components/chat/CodingAgentControlMenu.tsx',
  '/src/components/chat/ContextCapturePanel.tsx',
  '/src/components/chat/ContextViewerModal.tsx',
  '/src/components/chat/EventConditionPopover.tsx',
  '/src/components/chat/FrontendPreviewSection.tsx',
  '/src/components/chat/HandedOffCard.tsx',
  '/src/components/chat/MentionSuggestions.tsx',
  '/src/components/chat/MessageRoutePanel.tsx',
  '/src/components/chat/MicrophonePicker.tsx',
  '/src/components/chat/RungControl.tsx',
  '/src/components/chat/RungPicker.tsx',
  '/src/components/chat/StepDetailModal.tsx',
  '/src/components/chat/StepRecallList.tsx',
  '/src/components/chat/SuccessorCard.tsx',
  '/src/components/chat/ThreadUsers.tsx',
  '/src/components/chat/TodoListPanel.tsx',
  '/src/components/chat/contextGrouping.ts',
  '/src/components/chat/controlMenuPanelParts.ts',
  '/src/components/chat/controlMenuParts.ts',
  '/src/components/chat/deadPressProbe.ts',
  '/src/components/chat/loadStrippedSections.ts',
  '/src/components/chat/missingBodyReason.ts',
  '/src/components/chat/rungControlParts.ts',
  '/src/components/chat/rungLockedReason.ts',
  '/src/components/chat/rungRows.ts',
  '/src/components/chat/sectionTokens.ts',
  '/src/components/chat/toolCountBySource.ts',
  '/src/components/chat/useChatRungState.ts',
  '/src/components/chat/useRungState.ts',
  '/src/components/credentials/CredentialModal.tsx',
  '/src/components/credentials/SecretField.tsx',
  '/src/components/credentials/credentialAutofocus.ts',
  '/src/components/credentials/credentialScopes.ts',
  '/src/components/credentials/credentialSecret.ts',
  '/src/components/credentials/oauthClientForm.ts',
  '/src/components/credentials/providerConsoleHint.tsx',
  '/src/components/dashboard/Dashboard.tsx',
  '/src/components/dashboard/DashboardTile.tsx',
  '/src/components/dashboard/dashboardArrangement.ts',
  '/src/components/dashboard/useDashboardDrag.ts',
  '/src/components/dashboard/useDashboardSettled.ts',
  '/src/components/email/EmailConfirmModal.tsx',
  '/src/components/files/ChangeSelector.tsx',
  '/src/components/files/DiffView.tsx',
  '/src/components/files/FilePreviewModal.tsx',
  '/src/components/files/FileSearchModal.tsx',
  '/src/components/files/FilesView.tsx',
  '/src/components/files/FolderTree.tsx',
  '/src/components/files/LineNumberedCode.tsx',
  '/src/components/files/MarkdownDocument.tsx',
  '/src/components/files/PreviewImage.tsx',
  '/src/components/files/RenderedDiff.tsx',
  '/src/components/files/RepoFilePreview.tsx',
  '/src/components/files/RepoFilesView.tsx',
  '/src/components/files/RepoPreviewSplit.tsx',
  '/src/components/files/UrlPreviewInline.tsx',
  '/src/components/files/changeBadge.ts',
  '/src/components/files/diffStats.ts',
  '/src/components/files/fileSearch.ts',
  '/src/components/files/previewIframeLinks.ts',
  '/src/components/files/previewIframeShortcuts.ts',
  '/src/components/files/previewRevision.ts',
  '/src/components/files/previewSkeletons.tsx',
  '/src/components/files/repoPreviewSidebarWidth.ts',
  '/src/components/files/sideBySideDiff.tsx',
  '/src/components/layout/Drawer.tsx',
  '/src/components/layout/EngineBuildLogModal.tsx',
  '/src/components/layout/InlineForm.tsx',
  '/src/components/picker/PairingGate.tsx',
  '/src/components/picker/PairingScanner.tsx',
  '/src/components/plugins/AddOfficialMarketplaceButton.tsx',
  '/src/components/plugins/AffectedTriggerList.tsx',
  '/src/components/plugins/CatalogScanFailure.tsx',
  '/src/components/plugins/PluginDescription.tsx',
  '/src/components/plugins/PluginDetailPage.tsx',
  '/src/components/plugins/PluginFileList.tsx',
  '/src/components/plugins/PluginInstallPanel.tsx',
  '/src/components/plugins/PluginSection.tsx',
  '/src/components/plugins/PluginUninstallPanel.tsx',
  '/src/components/plugins/PluginsView.tsx',
  '/src/components/plugins/ProposeUpstreamButton.tsx',
  '/src/components/plugins/StoreTab.tsx',
  '/src/components/plugins/engineRequirement.ts',
  '/src/components/plugins/pluginCard.tsx',
  '/src/components/plugins/pluginContent.ts',
  '/src/components/plugins/pluginReadme.ts',
  '/src/components/search/SearchEverywhere.css',
  '/src/components/search/SearchEverywhere.tsx',
  '/src/components/search/menuIndex.ts',
  '/src/components/search/searchIndex.ts',
  '/src/components/search/searchSections.ts',
  '/src/components/search/titleMatch.ts',
  '/src/components/settings/BackgroundModelRow.tsx',
  '/src/components/settings/MemoryChoice.tsx',
  '/src/components/settings/ModelSelectionRow.tsx',
  '/src/components/settings/RemoteHeadersEditor.tsx',
  '/src/components/settings/TreeEstimate.tsx',
  '/src/components/settings/backgroundModels.ts',
  '/src/components/settings/judgmentBackend.ts',
  '/src/components/settings/providerCredential.ts',
  '/src/components/settings/remoteGatewayForm.ts',
  '/src/components/settings/usageProfiles.ts',
  '/src/components/shared/ArrivalList.tsx',
  '/src/components/shared/AutoTextarea.tsx',
  '/src/components/shared/ConfirmAndPromptDialogs.tsx',
  '/src/components/shared/ConfirmDialog.tsx',
  '/src/components/shared/ControlOptionList.tsx',
  '/src/components/shared/FormSkeleton.tsx',
  '/src/components/shared/HiddenFileInput.tsx',
  '/src/components/shared/ImagePopup.tsx',
  '/src/components/shared/ListRowAddCard.tsx',
  '/src/components/shared/ModelSelectionField.tsx',
  '/src/components/shared/ModelSelectionPicker.tsx',
  '/src/components/shared/PromptDialog.tsx',
  '/src/components/shared/ReleaseNoticeModal.tsx',
  '/src/components/shared/ScaleModal.tsx',
  '/src/components/shared/SecretInput.tsx',
  '/src/components/shared/ThreadOverflowItems.tsx',
  '/src/components/shared/arrivals.ts',
  '/src/components/shared/dialogKeyScope.ts',
  '/src/components/shared/mermaidBlocks.ts',
  '/src/components/shared/scaleSlider.ts',
  '/src/components/triggers/TriggerDetails.tsx',
  '/src/components/triggers/TriggerGroupHeader.tsx',
  '/src/components/triggers/TriggerItem.tsx',
  '/src/components/triggers/TriggersView.tsx',
  '/src/components/triggers/triggerScrollStep.ts',
  '/src/components/widgets/DashboardItem.tsx',
  '/src/components/widgets/WidgetFitCheck.tsx',
  '/src/components/widgets/WidgetMenu.tsx',
  '/src/components/widgets/WidgetPickerItems.tsx',
  '/src/components/widgets/WidgetSizePill.tsx',
  '/src/hooks/useAutoResizeTextarea.ts',
  '/src/hooks/useFocusWhenPlaced.ts',
  '/src/hooks/useInlineRename.ts',
  '/src/hooks/useModelSelection.ts',
  '/src/hooks/usePaneCentre.ts',
  '/src/hooks/useRowDrag.ts',
  '/src/hooks/useServerBackedField.ts',
  '/src/store/actions/app-media-bridge.ts',
  '/src/store/actions/app-update.ts',
  '/src/store/actions/archive-completed.ts',
  '/src/store/actions/chatgptPlan.ts',
  '/src/store/actions/claudeGateway.ts',
  '/src/store/actions/dashboards.ts',
  '/src/store/actions/engine-update.ts',
  '/src/store/actions/find-targets.ts',
  '/src/store/actions/frontend-preview.ts',
  '/src/store/actions/interactive-delivery.ts',
  '/src/store/actions/laneControls.ts',
  '/src/store/actions/remoteGatewayManage.ts',
  '/src/store/actions/themes.ts',
  '/src/store/actions/threads-delete.ts',
  '/src/store/actions/threads-detach.ts',
  '/src/store/actions/threads-drop.ts',
  '/src/store/actions/transcript-find.ts',
  '/src/store/actions/transcript-passages.ts',
  '/src/store/actions/update-relay.ts',
  '/src/store/actions/widget-actions.ts',
  '/src/store/actions/widgets.ts',
  '/src/store/backgroundChecks.ts',
  '/src/store/intelligenceLadderPrefs.ts',
  '/src/store/interactiveDelivery.ts',
  '/src/store/interactiveDeliveryDevice.ts',
  '/src/store/interactiveDeliveryRecords.ts',
  '/src/store/microphones.ts',
  '/src/store/pairingRequests.ts',
  '/src/store/restoreSourceActions.ts',
  '/src/store/updateRelay.ts',
  '/src/styles/chat/mermaid.css',
  '/src/utils/bindMode.ts',
  '/src/utils/csv.ts',
  '/src/utils/describeCron.ts',
  '/src/utils/exportThread.ts',
  '/src/utils/fileIcons.tsx',
  '/src/utils/frontmatter.ts',
  '/src/utils/headingSlug.ts',
  '/src/utils/lucidosVersion.ts',
  '/src/utils/markdownEngine.ts',
  '/src/utils/markdownImageSource.ts',
  '/src/utils/markdownLinkTooltip.ts',
  '/src/utils/markedConfig.ts',
  '/src/utils/mermaidFrame.html?raw',
  '/src/utils/mermaidFrame.ts',
  '/src/utils/mermaidLibrary.ts',
  '/src/utils/pairingCodeText.ts',
  '/src/utils/perfProbe.ts',
  '/src/utils/pinchGesture.ts',
  '/src/utils/recommendedCleanup.ts',
  '/src/utils/remoteContent.ts',
  '/src/utils/syntaxHighlight.ts',
  '/src/utils/taskListToggle.ts',
  '/src/utils/threadRef.ts',
  '/src/utils/updateChannel.ts',
  '/src/utils/urlFilter.ts',
  '/src/utils/workspaceOrder.ts',
];

export function smallLazyChunk(id: string): 'shell' | undefined {
  const normalized = id.replace(/\\/g, '/');
  return SHELL_MODULES.some(path => normalized.endsWith(path)) ? 'shell' : undefined;
}

type Chunk = BundleChunk & { code: string };
export function assertSmallLazyChunks(bundle: Record<string, Chunk | { type: 'asset' }>): void {
  const chunks = bundleChunks(bundle);
  const shell = findShellChunk(chunks);
  const byFile = new Map(chunks.map(c => [c.fileName, c]));
  const entryFiles = new Set<string>();
  const visit = (chunk: Chunk) => {
    if (entryFiles.has(chunk.fileName)) return;
    entryFiles.add(chunk.fileName);
    for (const file of chunk.imports) {
      const dependency = byFile.get(file);
      if (dependency) visit(dependency);
    }
  };
  chunks.filter(c => c.isEntry).forEach(visit);
  if (entryFiles.has(shell.fileName)) throw new Error('App shell is statically reachable from entry');
  const firstPaint = new Set(firstPaintChunks(bundle).map(c => c.fileName));
  for (const chunk of chunks) {
    if (chunk !== shell && chunk.moduleIds.some(id => smallLazyChunk(id))) {
      throw new Error(`Audited module escaped App shell: ${chunk.fileName}`);
    }
    if (firstPaint.has(chunk.fileName)) {
      const heavy = chunk.moduleIds.filter(id => !id.endsWith('?url')
        && /\/node_modules\/(?:marked|dompurify|highlight\.js|jsqr|mermaid)\//.test(id));
      if (heavy.length) {
        throw new Error(`Heavy library promoted into first paint: ${chunk.fileName}: ${heavy.join(', ')}`);
      }
    } else if (new TextEncoder().encode(chunk.code).length < 20_000) {
      throw new Error(`Deferred JavaScript must be at least 20,000 bytes: ${chunk.fileName}`);
    }
  }
}

export function smallLazyChunks(devBuildWatch = false): Plugin {
  return {
    name: 'lucidos-small-lazy-chunks',
    generateBundle: { order: 'post', handler(_options, bundle) {
      try {
        assertSmallLazyChunks(bundle);
      } catch (error) {
        if (!devBuildWatch && !this.meta.watchMode) throw error;
        this.warn(error instanceof Error ? error.message : String(error));
      }
    } },
  };
}
