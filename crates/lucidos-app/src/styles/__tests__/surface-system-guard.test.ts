import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { block, decl, rulesTargeting } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf8');

const css = {
  overlay: read('../global/modal-overlay.css'),
  surface: read('../global/surface.css'),
  host: read('../global/host-components.css'),
  shared: read('../global/shared-components.css'),
  components: read('../components.css'),
  mobile: read('../mobile.css'),
  steps: read('../steps.css'),
  mark: read('../header-mark.css'),
  popover: read('../global/anchored-popover.css'),
  input: read('../chat/input-messages.css'),
  previews: read('../panels/previews.css'),
  content: read('../panels/content.css'),
  search: read('../../components/search/SearchEverywhere.css'),
};

const rule = (sheet: string, selector: string): string => block(`\n${sheet}`, `\n${selector} {`);

describe('only a surface that blocks dims the app, and all of them alike', () => {
  it('draws every blocking scrim from the one token', () => {
    expect(decl(rule(css.overlay, '.modal-overlay'), 'background')).toBe('var(--scrim)');
    expect(decl(rule(css.overlay, '.ui-blocking-overlay'), 'background')).toBe('var(--scrim)');
    expect(decl(block(css.mobile, '.drawer-backdrop {'), 'background')).toBe('var(--scrim)');
  });

  it('dims nothing behind a menu or a palette', () => {
    expect(css.mark).not.toMatch(/\.brand-menu-scrim\b/);
    expect(decl(block(css.search, '.modal-overlay.search-everywhere-overlay {'), 'background')).toBe('none');
    expect(decl(rule(css.content, '.file-search-overlay'), 'background')).toBe('none');
  });
});

describe('every surface takes its corner and shadow from a token', () => {
  const surfaces: [string, string][] = [
    [css.surface, '.surface,\n.surface-box,\n.lucidos-select-menu'],
    [css.components, '.confirm-dialog'],
    [css.components, '.toast'],
    [css.components, '.file-preview-modal'],
    [css.steps, '.step-detail-modal'],
    [css.popover, '.anchored-popover'],
    [css.mark, '.brand-menu'],
    [css.shared, '.dropdown-menu'],
    [css.host, '.thread-overflow-menu'],
    [css.input, '.control-dropdown'],
    [css.input, '.image-attach-menu'],
    [css.input, '.camera-container'],
    [css.previews, '.message-route-panel'],
    [css.content, '.file-search-modal'],
  ];

  it.each(surfaces)('%#: names no literal radius, shadow or fill', (sheet, selector) => {
    const body = rule(sheet, selector);
    for (const prop of ['border-radius', 'box-shadow', 'background']) {
      const value = decl(body, prop);
      if (value === null) continue;
      expect(value, `${selector} ${prop}`).toMatch(/^var\(--/);
    }
  });
});

describe('menus wear the shared box instead of a copy of it', () => {
  const menus: [string, string][] = [
    [css.mark, '.brand-menu'],
    [css.host, '.thread-overflow-menu'],
    [css.shared, '.dropdown-menu'],
    [css.input, '.control-dropdown'],
    [css.input, '.image-attach-menu'],
    [css.previews, '.message-route-panel'],
  ];

  it.each(menus)('%#: declares no border, radius, shadow or fill of its own', (sheet, selector) => {
    const body = rule(sheet, selector);
    for (const prop of ['border', 'border-radius', 'box-shadow', 'background']) {
      expect(decl(body, prop), `${selector} ${prop}`).toBeNull();
    }
  });

  it.each([
    ['../../components/layout/HeaderMark.tsx', 'surface-box', 'brand-menu'],
    ['../../components/shared/OverflowMenu.tsx', 'surface-box', 'thread-overflow-menu'],
    ['../../components/shared/Dropdown.tsx', 'surface-box', 'dropdown-menu'],
    ['../../components/shared/ModelSelectionField.tsx', 'surface-box', 'dropdown-menu'],
    ['../../components/shared/NavChevron.tsx', 'surface-box', 'dropdown-menu'],
    ['../../components/files/ChangeSelector.tsx', 'surface-box', 'dropdown-menu'],
    ['../../components/chat/CodingAgentControlMenu.tsx', 'surface-box', 'control-dropdown'],
    ['../../components/chat/RungControl.tsx', 'surface-box', 'control-dropdown'],
    ['../../components/chat/PromptInput.tsx', 'surface-box', 'image-attach-menu'],
    ['../../components/chat/MessageRoutePanel.tsx', 'surface', 'message-route-panel'],
  ])('%s gives its panel the %s class before %s', (path, box, menu) => {
    expect(read(path)).toMatch(new RegExp(`['"\`]${box} (surface-raised )?${menu}\\b`));
  });

  it('keeps typography and the button size off the box alone', () => {
    const box = rule(css.surface, '.surface,\n.surface-box,\n.lucidos-select-menu');
    for (const prop of ['line-height', 'color', '--surface-inset']) {
      expect(decl(box, prop), prop).toBeNull();
    }
    expect(css.surface).not.toMatch(/\.surface-box \.action-btn/);
  });
});

describe('a head icon costs the body no width', () => {
  it('keys no body inset on the head having an icon', () => {
    expect(css.surface).not.toMatch(/\.surface-head:has\(\.surface-icon\)\s*\+\s*\.surface-body/);
  });

  it('gives the body the same left inset as the head', () => {
    expect(decl(rule(css.surface, '.surface-body'), 'padding')).toBe('0 var(--surface-inset) var(--surface-inset)');
    expect(decl(rule(css.surface, '.surface-head'), 'padding')).toMatch(/ var\(--surface-inset\)$/);
  });

  it('keeps the toast icon gutter on the heading, the one line beside the icon', () => {
    expect(decl(rule(css.components, '.toast-body'), 'padding-left')).toBeNull();
    expect(decl(rule(css.components, '.toast-heading'), 'padding-left')).toBe('calc(var(--toast-icon-size) + 0.5rem)');
  });
});

describe('a wrapped title keeps its first line where a short one sits', () => {
  it('derives the title padding from the head height, not the head padding', () => {
    expect(decl(rule(css.surface, '.surface-head'), 'min-height')).toBe('var(--surface-head-height)');
    expect(decl(rule(css.surface, '.surface-title'), 'padding-block'))
      .toBe('calc((var(--surface-head-height) - 2 * var(--space-xs) - 1lh) / 2)');
  });
});

describe('every surface insets its text by one token', () => {
  it('declares the inset once, on the surface itself', () => {
    expect(decl(rule(css.surface, '.surface'), '--surface-inset')).toBe('var(--space-lg)');
  });

  it.each([
    [css.surface, '.surface-foot', 'padding', '0 var(--surface-inset) var(--surface-inset)'],
    [css.popover, '.anchored-popover-body', 'padding', '0 var(--surface-inset) var(--surface-inset)'],
    [css.components, '.toast', '--toast-pad-x', 'var(--surface-inset)'],
  ])('%#: reads the token rather than restating a value', (sheet, selector, prop, value) => {
    expect(decl(rule(sheet, selector), prop)).toBe(value);
  });

  it('never re-pads a body to the head by hand', () => {
    expect(css.steps).not.toMatch(/\+\s*\.step-detail-body\s*\{[^}]*padding/);
  });
});

describe('the header palettes are one shape, over their own pane', () => {
  it('draws both input rows as a surface head, restating none of it', () => {
    expect(read('../../components/search/SearchEverywhere.tsx')).toContain('class="surface-head search-everywhere-header"');
    expect(read('../../components/files/FileSearchModal.tsx')).toContain('class="surface-head file-search-header"');
    for (const [sheet, selector] of [[css.search, '.search-everywhere-header'], [css.content, '.file-search-header']]) {
      expect(decl(rule(sheet, selector), 'padding'), selector).toBeNull();
    }
  });

  it('hangs file search from the header on the shared line', () => {
    expect(decl(rule(css.content, '.file-search-overlay'), 'padding'))
      .toMatch(/^calc\(var\(--app-header-bottom\) \+ var\(--header-surface-gap\)\)/);
  });

  it('gives the menu and file search the one shared width', () => {
    expect(decl(rule(css.mark, '.brand-menu'), 'width')).toBe('var(--header-surface-width)');
    expect(decl(rule(css.content, '.file-search-modal'), 'max-width')).toMatch(/^min\(var\(--header-surface-width\),/);
  });

  it('widens Search everywhere past the menu on desktop, but not on a phone', () => {
    const modal = rulesTargeting(css.search, 'search-everywhere-modal').filter(r => r.props.has('max-width'));
    const desktop = modal.filter(r => r.atRules === '');
    expect(desktop).toHaveLength(1);
    expect(desktop[0].props.get('max-width')).not.toMatch(/--header-surface-width/);
    expect(desktop[0].props.get('max-width')).toMatch(/^min\(\d+(\.\d+)?rem,/);
    const last = modal[modal.length - 1];
    expect(last.atRules).toBe('@media (--phone-layout)');
    expect(last.props.get('max-width')).toMatch(/^min\(var\(--header-surface-width\),/);
  });

  it('narrows both to their pane, so neither straddles the divider', () => {
    expect(decl(rule(css.search, '.search-everywhere-modal'), 'max-width')).toMatch(/var\(--pane-fit, 100%\)/);
    expect(decl(rule(css.content, '.file-search-modal'), 'max-width')).toMatch(/var\(--pane-fit, 100%\)/);
  });

  it('centres both over a pane through the shared shift', () => {
    for (const path of ['../../components/search/SearchEverywhere.tsx', '../../components/files/FileSearchModal.tsx']) {
      const source = read(path);
      expect(source, path).toMatch(/surface-pane-centred/);
      expect(source, path).toMatch(/usePaneCentre\(/);
    }
  });
});

describe('a surface closes from the X in its head, never from a text button', () => {
  const sources = [
    '../../components/chat/StepDetailModal.tsx',
    '../../components/chat/CheckpointDiffModal.tsx',
    '../../components/chat/ContextViewerModal.tsx',
    '../../components/chat/EventConditionPopover.tsx',
    '../../components/shared/Explainer.tsx',
    '../../components/files/FilePreviewModal.tsx',
    '../../components/settings/DirectoryPicker.tsx',
    '../../components/settings/ResponseStylesSection.tsx',
  ].map((p) => [p, read(p)] as const);

  it.each(sources)('%s has no bottom Close button', (_path, source) => {
    expect(source).not.toMatch(/step-detail-close|explainer-actions|>\s*Close\s*</);
    expect(source).toContain('<SurfaceHead');
  });

  it.each([
    '../../components/shared/Surface.tsx',
    '../../components/shared/Toast.tsx',
    '../../components/search/SearchEverywhere.tsx',
    '../../components/files/FileSearchModal.tsx',
    '../../components/layout/BackupReminderBanner.tsx',
    '../../components/layout/SlownessBanner.tsx',
  ])('%s gives its close X a tooltip matching its name', (path) => {
    const buttons = read(path).split('<button').slice(1)
      .map((chunk: string) => chunk.split('</button>')[0])
      .filter((chunk: string) => /\b(?:surface-close|toast-close)\b/.test(chunk));
    expect(buttons.length, 'found the close button').toBeGreaterThan(0);
    for (const button of buttons) {
      const name = button.match(/aria-label=(\{[^}]+\}|"[^"]+")/)?.[1];
      expect(name, button).toBeTruthy();
      expect(button, 'tooltip mirrors the aria-label').toContain(`data-tooltip=${name}`);
    }
  });

  it('draws the shared close glyph, not a hand-rolled one', () => {
    expect(read('../../components/files/FileSearchModal.tsx')).toContain('<CloseIcon />');
  });
});
