import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../Explainer.tsx'), 'utf-8');
const filterPanel: string = readFileSync(
  resolve(here, '../../layout/ThreadFilterPanel.tsx'),
  'utf-8',
);
const hostCss: string = readFileSync(
  resolve(here, '../../../styles/global/host-components.css'),
  'utf-8',
);
const sharedCss: string = readFileSync(
  resolve(here, '../../../styles/global/shared-components.css'),
  'utf-8',
);
const drawerCss: string = readFileSync(resolve(here, '../../../styles/drawer.css'), 'utf-8');
const pagesCss: string = readFileSync(resolve(here, '../../../styles/pages.css'), 'utf-8');
const triggerDetails: string = readFileSync(
  resolve(here, '../../triggers/TriggerDetails.tsx'),
  'utf-8',
);
const credentialModal: string = readFileSync(
  resolve(here, '../../credentials/CredentialModal.tsx'),
  'utf-8',
);
const settingsView: string = readFileSync(
  resolve(here, '../../settings/SettingsView.tsx'),
  'utf-8',
);

describe('Explainer: the trigger button', () => {
  it('is a non-submitting button', () => {
    expect(source).toMatch(/<button[\s\S]*?type="button"[\s\S]*?class="icon-btn explainer-btn"/);
  });

  it('has an accessible name derived from the title', () => {
    expect(source).toMatch(/aria-label=\{`About \$\{title\}`\}/);
  });

  it('uses no native tooltip', () => {
    expect(source).not.toMatch(/data-tooltip=/);
    const buttonAttrs = source.match(/<button\b([\s\S]*?)>\s*\n\s*<InfoIcon/);
    expect(buttonAttrs, 'found the trigger button').not.toBeNull();
    expect(buttonAttrs![1]).not.toMatch(/\stitle=/);
  });
});

describe('Explainer: delegates the dismiss contract', () => {
  it('builds its overlay through <Overlay>, passing the icon button as anchor', () => {
    expect(source).toMatch(/<Overlay\b/);
    expect(source).toMatch(/anchor=\{anchor\}/);
  });

  it('never hand-rolls its own dismiss listener', () => {
    expect(source).not.toMatch(/addEventListener\(\s*['"](?:pointerdown|mousedown|click)['"]/);
  });
});

describe('Explainer: opens at its icon, like the Waits panel', () => {
  it('is an anchored popover with no scrim', () => {
    expect(source).toMatch(/backdrop=\{false\}/);
    expect(source).not.toMatch(/confirm-dialog/);
    expect(source).toMatch(/useAnchoredPosition\(open \? anchor : null, panelRef, '\.pane, \.thread-drawer'\)/);
  });

  it('moves focus in only once the popover is placed', () => {
    expect(source).toMatch(/if \(placed\) panelRef\.current\?\.querySelector/);
    expect(source).toMatch(/\}, \[placed\]\);/);
  });

  it('caps its width to the pane it was measured against', () => {
    expect(source).toMatch(/'--anchored-popover-fit': `\$\{pos\.maxWidth\}px`/);
    const rule = hostCss.match(/\.explainer-popover \{[\s\S]*?\n\}/);
    expect(rule, 'found the .explainer-popover rule').not.toBeNull();
    expect(rule![0]).toMatch(/width:\s*min\([^;]*var\(--anchored-popover-fit\)/);
  });

  it('shares the anchored popover shell with the Waits panel, not a copy of it', () => {
    expect(source).toMatch(/panelClass=\{`surface anchored-popover explainer-popover\$\{wide \? ' explainer-popover-wide' : ''\}\$\{protectedClassFrom\(anchor\)\}`\}/);
    expect(source).toMatch(/class="surface-body anchored-popover-body explainer-body"/);
    const rule = hostCss.match(/\.explainer-popover \{[\s\S]*?\n\}/)![0];
    for (const prop of ['position', 'z-index', 'display', 'overflow']) {
      expect(rule, `.explainer-popover restates the shell's ${prop}`).not.toMatch(new RegExp(`\\n\\s*${prop}:`));
    }
  });
});

describe('Explainer: the popover escapes its host markup', () => {
  it('portals the popover to <body>', () => {
    expect(source).toMatch(/\n\s*portal\n/);
  });
});

describe('Explainer: host-only, never served to app iframes', () => {
  it('styles the explainer in host-components.css', () => {
    expect(hostCss).toMatch(/\.explainer-btn\b/);
    expect(hostCss).toMatch(/\.explainer-slot\b/);
    expect(hostCss).toMatch(/\.explainer-body\b/);
  });

  it('keeps the classes out of the stylesheet the engine serves to apps', () => {
    expect(sharedCss).not.toMatch(/explainer/);
  });
});

describe('Explainer: the icon never wraps away from its label', () => {
  it('glues the button to the label with a word joiner', () => {
    expect(source).toMatch(/<span class="explainer-slot">\s*<button/);
    expect(hostCss).toMatch(/\.explainer-slot::before \{\s*content:\s*'\\2060';/);
  });

  it('generates the joiner, so it stays out of the text and the accname', () => {
    expect(source).not.toMatch(/\\u2060/);
  });

  it('keeps the slot a plain inline box, which is what exposes the joiner', () => {
    const rule = hostCss.match(/\.explainer-slot \{[\s\S]*?\n\}/);
    expect(rule, 'found the .explainer-slot rule').not.toBeNull();
    expect(rule![0]).toMatch(/display:\s*inline;/);
    expect(rule![0]).toMatch(/line-height:\s*0;/);
  });

  it('puts the space before the icon on the slot, where a gap row zeroes it', () => {
    expect(hostCss).toMatch(/\.explainer-slot \{[\s\S]*?margin-left:\s*0\.375rem/);
    const btn = hostCss.match(/\.explainer-btn \{[\s\S]*?\n\}/);
    expect(btn, 'found the .explainer-btn rule').not.toBeNull();
    expect(btn![0]).not.toMatch(/margin-left/);
    expect(drawerCss).toMatch(/\.thread-filter-option > \.explainer-slot \{\s*margin-left:\s*0;/);
    expect(pagesCss).toMatch(/\.form-checkbox-row > \.explainer-slot \{\s*margin-left:\s*0;/);
  });
});

describe("Explainer: the glyph sits on the label's cap height", () => {
  it('corrects `vertical-align: middle`, which centres on the x-height', () => {
    const rule = hostCss.match(/\.explainer-btn \{[\s\S]*?\n\}/);
    expect(rule, 'found the .explainer-btn rule').not.toBeNull();
    expect(rule![0]).toMatch(/vertical-align:\s*middle/);
    expect(rule![0]).toMatch(/\n\s*top:\s*-0?\.\d+em;/);
  });
});

describe('Explainer: the popover is a named, focus-trapping dialog', () => {
  it('names the panel, so a screen reader does not meet an unnamed dialog', () => {
    expect(source).toMatch(/panelProps=\{\{\s*'aria-label':\s*title\s*\}\}/);
  });

  it('renders through <Overlay>, which owns its Tab containment and focus restore', () => {
    expect(source).toMatch(/<Overlay\b/);
    expect(source).not.toMatch(/addEventListener\('keydown'/);
  });
});

describe("FieldLabel: the button never becomes the label's own control", () => {
  it('renders the explainer OUTSIDE the <label>', () => {
    expect(source).toMatch(/<label>\{label \?\? title\}<\/label>\s*\n\s*\{children \? <Explainer/);
  });

  it('is what the sibling-control forms use, so no bare <label> wraps an Explainer', () => {
    for (const [name, src] of [
      ['TriggerDetails', triggerDetails],
      ['CredentialModal', credentialModal],
    ] as const) {
      const between = /<label>(?:(?!<\/label>)[\s\S])*?<Explainer/.test(src);
      expect(between, `${name} puts an Explainer inside a <label>`).toBe(false);
    }
  });

  it('lets a checkbox row keep its explainer inline, where the input wins', () => {
    expect(filterPanel).toMatch(
      /<label class=[^>]*thread-filter-option[^>]*>[\s\S]*?<input[\s\S]*?<Explainer title="Include deleted">/,
    );
  });
});

describe('Explainer: sections of the same shape place the icon the same way', () => {
  it('puts the push explanation on the switch, not on the heading above it', () => {
    const heading = settingsView.match(
      /data-search-anchor="appearance:notifications"[\s\S]*?<\/div>/,
    );
    expect(heading, 'found the Notifications section title').not.toBeNull();
    expect(heading![0]).not.toMatch(/<Explainer/);
    expect(settingsView).toMatch(
      /<span class="settings-row-label">\s*Push notifications[\s\S]{0,400}?<Explainer title="Push notifications">/,
    );
  });

  it('leaves the Links heading bare too, since its rows carry their own', () => {
    const heading = settingsView.match(/data-search-anchor="appearance:links"[\s\S]*?<\/div>/);
    expect(heading, 'found the Links section title').not.toBeNull();
    expect(heading![0]).not.toMatch(/<Explainer/);
    expect(settingsView).toMatch(
      /<span class="settings-row-label">\s*Open links in\s*<Explainer title="Open links in">/,
    );
  });
});

describe('Explainer: the thread filter panel is its first consumer', () => {
  it('explains "Include deleted", which had no explanation at all', () => {
    expect(filterPanel).toMatch(/<Explainer title="Include deleted">/);
  });
});
