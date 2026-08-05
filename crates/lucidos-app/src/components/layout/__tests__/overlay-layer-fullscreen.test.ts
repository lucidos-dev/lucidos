import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { overlayLayerTarget } from '../OverlayLayer';
import { fullscreenBlocksHostOverlays, resolveOverlayMount } from '../../../store/appFullscreenHost';

// ─────────────────────────────────────────────────────────────────────────────
// Host overlays over a fullscreen app. A natively fullscreen element is painted
// ALONE: the browser renders that element's subtree and nothing else, at any
// z-index. Every host overlay is mounted at the app root, outside it, so an
// app's lucidos.ui.previewFile / confirm / prompt / toast showed nothing while
// the app was fullscreen and the promise resolved anyway.
//
// The fix is to move the whole group INTO the fullscreen element, which is only
// possible because fullscreen is now requested on the app PANEL rather than on
// the iframe (an iframe renders no DOM children, so there was nowhere to put
// them). The frontend test environment is deliberately non-jsdom, so the pure
// decision is unit-tested and the wiring is pinned in source, the same approach
// as AppUiInline.test.ts.
// ─────────────────────────────────────────────────────────────────────────────

const here: string = dirname(fileURLToPath(import.meta.url));
const src = (rel: string): string => readFileSync(resolve(here, rel), 'utf-8');

const appSrc = src('../../../App.tsx');
const layerSrc = src('../OverlayLayer.tsx');
const appUiSrc = src('../../apps/AppUiInline.tsx');
const headerSrc = src('../ContentHeaderActions.tsx');
const appsActionsSrc = src('../../../store/actions/apps.ts');
const startupSrc = src('../../../hooks/useStartup.ts');
const modalCss = src('../../../styles/global/modal-overlay.css');
const previewsCss = src('../../../styles/panels/previews.css');

describe('overlayLayerTarget', () => {
  const mount = { nodeName: 'DIV' } as unknown as HTMLElement;

  // Nothing fullscreen is the overwhelmingly common case, and it must produce
  // the DOM the app root had before this component existed: no wrapper element,
  // no portal, no new stacking context.
  it('renders inline when no app is natively fullscreen', () => {
    expect(overlayLayerTarget(null, false)).toBe('inline');
  });

  it('portals into the mount inside the fullscreen app panel', () => {
    expect(overlayLayerTarget(mount, false)).toBe(mount);
  });

  // A client refresh has to find the toast where UiBlockingOverlay's carve-out
  // expects it: that blocker inerts its own SIBLINGS and skips
  // `.toast-container` by class, and a portaled toast is not a sibling but a
  // descendant of `.app-shell`, which the blocker inerts wholesale. `inert`,
  // unlike `pointer-events`, cannot be overridden by a descendant, so the
  // "Refreshing…" toast would be visible and dead.
  it('stands down during a client refresh, even with an app fullscreen', () => {
    expect(overlayLayerTarget(mount, true)).toBe('inline');
  });
});

describe('resolveOverlayMount', () => {
  const mount = { nodeName: 'DIV' } as unknown as HTMLElement;
  /** A stand-in for the fullscreen element: whether it is an app panel, and
   *  what it holds. */
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

  // An app that calls requestFullscreen on its own content makes the IFRAME the
  // host document's fullscreen element, and an iframe renders no DOM children.
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

  // The host drives this one: its own panel is the fullscreen element, so the
  // mount is inside it and the overlay layer is painted with the app.
  it('does not block when a mount resolved inside the fullscreen element', () => {
    expect(fullscreenBlocksHostOverlays(el(), mount)).toBe(false);
  });

  // Nowhere to paint: the honest answer is a refusal, not a modal nobody can
  // see plus a resolved promise.
  it('blocks when something the host cannot render into is fullscreen', () => {
    expect(fullscreenBlocksHostOverlays(el(), null)).toBe(true);
  });

  // A detached fullscreen element is a fullscreen that is ENDING: the panel
  // remounted under it and the browser has not caught up. The shell is already
  // on its way back to the normal layout, where the overlay is perfectly
  // visible, so refusing there rejects a preview that is about to work.
  it('does not block on a detached fullscreen element', () => {
    expect(fullscreenBlocksHostOverlays(el(false), null)).toBe(false);
  });
});

describe('the overlay group is wired through the layer', () => {
  /** The children of the single <OverlayLayer> element in App.tsx. */
  const group = appSrc.match(/<OverlayLayer>([\s\S]*?)<\/OverlayLayer>/)?.[1] ?? '';

  it('wraps the app-facing host surfaces', () => {
    expect(group).not.toBe('');
    for (const slot of [
      'ConfirmDialog', 'PromptDialog', 'FilePreviewModalSlot', 'Toast',
      'SearchEverywhereSlot', 'ScaleModalSlot', 'FileSearchModalSlot',
      'ImagePopupSlot', 'MessageRoutePanelSlot', 'StepDetailModalSlot',
    ]) {
      expect(group).toContain(`<${slot} />`);
    }
  });

  // UiBlockingOverlay inerts its own SIBLINGS (overlay.parentElement's
  // children). Inside the portal it would inert the nine overlays beside it and
  // miss `.app-shell` entirely, i.e. stop blocking the thing it exists to
  // block. DropZone renders null, so moving it costs a remount and buys nothing.
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

  // The sync has NO dep array, so a remount is covered by the render that
  // caused it: the browser can still be holding the old detached panel as
  // `document.fullscreenElement`, and only a re-derivation notices.
  it('re-asserts on every render, not only on the fullscreen event', () => {
    expect(appUiSrc).toMatch(/useLayoutEffect\(\(\) => \{\s*\n\s*if \(isActiveLayout\) syncAppFullscreenHost\(\);\s*\n\s*\}\);/);
  });

  // The listeners and the on-unmount clear are keyed SEPARATELY. Sharing the
  // dep-less effect meant every render tore them down and wrote null to the
  // host on the way, churning the portal target (and any open modal with it)
  // between two renders that both had a fullscreen app.
  it('clears the host on unmount, not between renders', () => {
    const listeners = appUiSrc.match(/useLayoutEffect\(\(\) => \{\n {4}if \(!isActiveLayout\) return;\n {4}document\.addEventListener[\s\S]*?\n {2}\}, \[isActiveLayout\]\);/)?.[0] ?? '';
    expect(listeners).not.toBe('');
    expect(listeners).toContain('appFullscreenHost.value = null');
  });

  // The mount is a dedicated EMPTY element, not the panel: the panel's children
  // are Preact's (the iframe, the pseudo chrome), and a portal filling a
  // container another component also fills asks two diffs to agree about DOM
  // they each think they own.
  it('renders an empty mount inside the panel', () => {
    expect(appUiSrc).toContain('<div class="app-overlay-layer" data-overlay-layer="" />');
  });

  // The fullscreen request has to resolve the same panel the mount lives in.
  it('marks the panel with the role the fullscreen request resolves', () => {
    expect(appUiSrc).toContain('data-role="app-ui-panel"');
    expect(appsActionsSrc).toMatch(/getVisibleAppPanel[\s\S]{0,400}app-ui-panel/);
  });

  // The refusal and the portal that acts on it have to be reading the same
  // instant, or the host refuses a preview it could show (or, worse, shows one
  // nobody can see). The bridge re-derives immediately before deciding.
  it('re-derives in the app bridge before deciding whether it can show', () => {
    const branch = startupSrc.match(/if \(data\.type === 'lucidos:ui:preview-file'\) \{[\s\S]*?\n {6}\}/)?.[0] ?? '';
    expect(branch).not.toBe('');
    const syncAt = branch.indexOf('syncAppFullscreenHost()');
    const decideAt = branch.indexOf('filePreviewBlockedReason()');
    expect(syncAt).toBeGreaterThan(-1);
    expect(decideAt).toBeGreaterThan(syncAt);
  });
});

describe('native fullscreen is requested on the panel, not the iframe', () => {
  // The bug: with the IFRAME fullscreen the browser paints only the iframe, and
  // an iframe has no DOM children, so no arrangement of the host's overlays can
  // be seen. The panel wraps the iframe and can hold them.
  it('binds the request to the panel', () => {
    expect(headerSrc).toMatch(/const panel = getVisibleAppPanel\(\)/);
    expect(headerSrc).toMatch(/anyPanel\.requestFullscreen[\s\S]{0,80}\.bind\(panel\)/);
    expect(headerSrc).toMatch(/anyPanel\.webkitRequestFullscreen[\s\S]{0,80}\.bind\(panel\)/);
  });

  // Fullscreen moves focus to the element that requested it; the app is what
  // the user is about to type into.
  it('still puts keyboard focus in the app frame', () => {
    expect(headerSrc).toMatch(/then\(\(\) => frame\.focus\(\)\)/);
  });

  // Derived from the visible frame rather than by a second query, so the two
  // cannot disagree about which app is on screen during a layout swap.
  it('resolves the panel from the visible frame', () => {
    expect(appsActionsSrc).toMatch(/getVisibleAppPanel[^}]*getVisibleAppFrame\(\)\?\.closest/);
  });
});

describe('the portaled layer survives the inert-behind rule', () => {
  // Portaled, the layer sits inside `.app-shell`, whose children go
  // pointer-events:none while any overlay is open. Without the exemption the
  // scrim and the toast (which render OUTSIDE `.app-shell` normally) would go
  // inert with it: a backdrop click would not dismiss and a toast could not be
  // dismissed.
  it('re-enables pointer events for the layer', () => {
    const exemption = modalCss.match(
      /:root\[data-overlay-open\][^{]*\[data-overlay-panel\][^{]*\{[^}]*pointer-events:\s*auto/,
    )?.[0] ?? '';
    expect(exemption).toContain('[data-overlay-layer]');
  });

  // The mount must add no box: its children are position:fixed scrims and the
  // toast container, and the app panel is a flex column, so a real box would be
  // a flex item and (worse) a hit target over the app.
  it('generates no box of its own', () => {
    expect(modalCss).toMatch(/\.app-overlay-layer\s*\{[^}]*display:\s*contents/);
  });

  // One mount, owned by the panel that has to contain it; the layer is a pure
  // portal into it and adds no second wrapper.
  it('is the element the panel renders, filled by the layer', () => {
    expect(appUiSrc).toContain('class="app-overlay-layer" data-overlay-layer=""');
    expect(layerSrc).toContain('createPortal(<>{children}</>, target)');
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

  // The pseudo-fullscreen half of the bug, and a pure stacking one: the panel
  // sat at --z-tooltip (10000), above every modal and every toast.
  it('sits below the modal layer, so host overlays paint over the app', () => {
    expect(panelToken).toBe('z-app-fullscreen');
    expect(TOKENS['z-app-fullscreen']).toBeLessThan(TOKENS['z-modal']);
    expect(TOKENS['z-app-fullscreen']).toBeLessThan(TOKENS['z-toast']);
  });

  // The other half of the requirement: fullscreen still means fullscreen, so
  // the app must cover the floating header chrome.
  it('sits above the header chrome, so the app still owns the screen', () => {
    expect(TOKENS['z-app-fullscreen']).toBeGreaterThan(TOKENS['z-control-panel']);
  });

  // A selector list is invalidated as a whole by one unrecognized
  // pseudo-class, so the prefixed and unprefixed forms cannot be grouped: an
  // engine that knows only one of them would drop both and paint the panel at
  // its in-pane size over a black backdrop.
  it('sizes and paints the natively fullscreen panel, in two separate rules', () => {
    for (const sel of ['\\.app-ui-inline:fullscreen', '\\.app-ui-inline:-webkit-full-screen']) {
      const block = previewsCss.match(new RegExp(`${sel}\\s*\\{[^}]*\\}`))?.[0] ?? '';
      expect(block, `${sel} rule missing`).not.toBe('');
      expect(block).toMatch(/background:\s*var\(--bg-primary\)/);
      expect(block).toMatch(/inset:\s*0/);
    }
  });
});
