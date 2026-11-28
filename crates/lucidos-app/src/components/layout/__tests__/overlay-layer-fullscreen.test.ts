import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { overlayLayerTarget } from '../OverlayLayer';
import { fullscreenBlocksHostOverlays, resolveOverlayMount } from '../../../store/appFullscreenHost';


const here: string = dirname(fileURLToPath(import.meta.url));
const src = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const appSrc = src('../../../App.tsx');
const layerSrc = src('../OverlayLayer.tsx');
const appUiSrc = src('../../apps/AppUiInline.tsx');
const appsActionsSrc = src('../../../store/actions/apps.ts');
const appFrameMessagesSrc = src('../../../store/actions/app-frame-messages.ts');
const modalCss = src('../../../styles/global/modal-overlay.css');
const previewsCss = src('../../../styles/panels/previews.css');

describe('overlayLayerTarget', () => {
  const mount = { nodeName: 'DIV' } as unknown as HTMLElement;

  it('sits at its home beside the app shell when no app is natively fullscreen', () => {
    expect(overlayLayerTarget(null, false)).toBe('home');
  });

  it('moves into the mount inside the fullscreen app panel', () => {
    expect(overlayLayerTarget(mount, false)).toBe(mount);
  });

  it('stays home during a client refresh, even with an app fullscreen', () => {
    expect(overlayLayerTarget(mount, true)).toBe('home');
  });
});

describe('resolveOverlayMount', () => {
  const mount = { nodeName: 'DIV' } as unknown as HTMLElement;
  const el = (opts: { panel: boolean; mount?: HTMLElement | null; isConnected?: boolean }) => ({
    isConnected: opts.isConnected ?? true,
    matches: (sel: string) => opts.panel && sel === '[data-role="app-ui-panel"]',
    querySelector: () => opts.mount ?? null,
  }) as unknown as Element;

  it('is null when nothing is fullscreen', () => {
    expect(resolveOverlayMount(null)).toBeNull();
  });

  it('finds the mount inside a fullscreen app panel', () => {
    expect(resolveOverlayMount(el({ panel: true, mount }))).toBe(mount);
  });

  it('is null when the fullscreen element is not an app panel', () => {
    expect(resolveOverlayMount(el({ panel: false, mount }))).toBeNull();
  });

  it('is null for a detached fullscreen element', () => {
    expect(resolveOverlayMount(el({ panel: true, mount, isConnected: false }))).toBeNull();
  });
});

describe('fullscreenBlocksHostOverlays', () => {
  const mount = { nodeName: 'DIV' } as unknown as HTMLElement;
  const el = (isConnected = true) => ({ isConnected }) as unknown as Element;

  it('does not block when nothing is fullscreen', () => {
    expect(fullscreenBlocksHostOverlays(null, null)).toBe(false);
  });

  it('does not block when a mount resolved inside the fullscreen element', () => {
    expect(fullscreenBlocksHostOverlays(el(), mount)).toBe(false);
  });

  it('blocks when something the host cannot render into is fullscreen', () => {
    expect(fullscreenBlocksHostOverlays(el(), null)).toBe(true);
  });

  it('does not block on a detached fullscreen element', () => {
    expect(fullscreenBlocksHostOverlays(el(false), null)).toBe(false);
  });
});

describe('the overlay group is wired through the layer', () => {
  const group = appSrc.match(/<OverlayLayer>([\s\S]*?)<\/OverlayLayer>/)?.[1] ?? '';

  it('wraps the app-facing host surfaces', () => {
    expect(group).not.toBe('');
    for (const slot of [
      'ConfirmAndPromptSlot', 'FilePreviewModalSlot', 'Toast',
      'SearchEverywhereSlot', 'ScaleModalSlot', 'FileSearchModalSlot',
      'ImagePopupSlot', 'MessageRoutePanelSlot', 'StepDetailModalSlot',
      'CameraCaptureSlot',
    ]) {
      expect(group).toContain(`<${slot} />`);
    }
  });

  it('leaves the blocker and the drop dispatcher at the app root', () => {
    expect(group).not.toContain('UiBlockingOverlay');
    expect(group).not.toContain('DropZone');
    expect(appSrc).toContain('<UiBlockingOverlay />');
    expect(appSrc).toContain('<DropZone />');
  });
});

describe('the fullscreen host is kept in step with the DOM', () => {
  it('re-derives on both spellings of the fullscreen change event', () => {
    expect(appUiSrc).toContain("addEventListener('fullscreenchange', syncAppFullscreenHost)");
    expect(appUiSrc).toContain("addEventListener('webkitfullscreenchange', syncAppFullscreenHost)");
  });

  it('re-asserts on every render, not only on the fullscreen event', () => {
    expect(appUiSrc).toMatch(/useLayoutEffect\(\(\) => \{\s*\n\s*if \(isActiveLayout\) syncAppFullscreenHost\(\);\s*\n\s*\}\);/);
  });

  it('clears the host on unmount, not between renders', () => {
    const listeners = appUiSrc.match(/useLayoutEffect\(\(\) => \{\n {4}if \(!isActiveLayout\) return;\n {4}document\.addEventListener[\s\S]*?\n {2}\}, \[isActiveLayout\]\);/)?.[0] ?? '';
    expect(listeners).not.toBe('');
    expect(listeners).toContain('appFullscreenHost.value = null');
  });

  it('renders an empty mount inside the panel', () => {
    expect(appUiSrc).toContain('<div class="app-overlay-layer" data-overlay-layer="" />');
  });

  it('marks the panel with the role the fullscreen request resolves', () => {
    expect(appUiSrc).toContain('data-role="app-ui-panel"');
    expect(appsActionsSrc).toMatch(/getVisibleAppPanel[\s\S]{0,400}app-ui-panel/);
  });

  it('re-derives in the app bridge before deciding whether it can show', () => {
    const branch = appFrameMessagesSrc.match(/if \(data\.type === 'lucidos:ui:preview-file'\) \{[\s\S]*?\n {4}\}/)?.[0] ?? '';
    expect(branch).not.toBe('');
    const syncAt = branch.indexOf('syncAppFullscreenHost()');
    const decideAt = branch.indexOf('filePreviewBlockedReason()');
    expect(syncAt).toBeGreaterThan(-1);
    expect(decideAt).toBeGreaterThan(syncAt);
  });
});

describe('native fullscreen is requested on the panel, not the iframe', () => {
  it('binds the request to the panel', () => {
    expect(appsActionsSrc).toMatch(/const panel = getVisibleAppPanel\(\)/);
    expect(appsActionsSrc).toMatch(/anyPanel\.requestFullscreen[\s\S]{0,80}\.bind\(panel\)/);
    expect(appsActionsSrc).toMatch(/anyPanel\.webkitRequestFullscreen[\s\S]{0,80}\.bind\(panel\)/);
  });

  it('still puts keyboard focus in the app frame', () => {
    expect(appsActionsSrc).toMatch(/then\(\(\) => frame\.focus\(\)\)/);
  });

  it('resolves the panel from the visible frame', () => {
    expect(appsActionsSrc).toMatch(/getVisibleAppPanel[^}]*getVisibleAppFrame\(\)\?\.closest/);
  });
});

describe('the portaled layer survives the inert-behind rule', () => {
  it('re-enables pointer events for the layer', () => {
    const exemption = modalCss.match(
      /:root\[data-overlay-open\][^{]*\[data-overlay-panel\][^{]*\{[^}]*pointer-events:\s*auto/,
    )?.[0] ?? '';
    expect(exemption).toContain('[data-overlay-layer]');
  });

  it('generates no box of its own', () => {
    expect(modalCss).toMatch(/\.app-overlay-layer\s*\{[^}]*display:\s*contents/);
  });

  it('is the element the panel renders, and the layer moves its node into it', () => {
    expect(appUiSrc).toContain('class="app-overlay-layer" data-overlay-layer=""');
    expect(layerSrc).toContain('createPortal(<>{children}</>, overlayLayerNode())');
    expect(layerSrc).not.toContain('class="app-overlay-layer"');
  });
});

describe('a fullscreen app panel is layered below the host overlays', () => {
  const TOKENS: Record<string, number> = {};
  for (const m of src('../../../styles/global/base.css').matchAll(/--(z-[\w-]+):\s*(\d+)\s*;/g)) {
    TOKENS[m[1]] = parseInt(m[2], 10);
  }
  const panelBlock = previewsCss.match(/\.app-ui-inline\.app-ui-fullscreen\s*\{[^}]*\}/)?.[0] ?? '';
  const panelToken = panelBlock.match(/z-index:\s*var\(--([\w-]+)\)/)?.[1] ?? '';

  it('sits below the modal layer, so host overlays paint over the app', () => {
    expect(panelToken).toBe('z-app-fullscreen');
    expect(TOKENS['z-app-fullscreen']).toBeLessThan(TOKENS['z-modal']);
    expect(TOKENS['z-app-fullscreen']).toBeLessThan(TOKENS['z-toast']);
  });

  it('sits above the header chrome, so the app still owns the screen', () => {
    expect(TOKENS['z-app-fullscreen']).toBeGreaterThan(TOKENS['z-control-panel']);
  });

  it('sizes and paints the natively fullscreen panel, in two separate rules', () => {
    for (const sel of ['\\.app-ui-inline:fullscreen', '\\.app-ui-inline:-webkit-full-screen']) {
      const block = previewsCss.match(new RegExp(`${sel}\\s*\\{[^}]*\\}`))?.[0] ?? '';
      expect(block, `${sel} rule missing`).not.toBe('');
      expect(block).toMatch(/background:\s*var\(--bg-primary\)/);
      expect(block).toMatch(/inset:\s*0/);
    }
  });
});
