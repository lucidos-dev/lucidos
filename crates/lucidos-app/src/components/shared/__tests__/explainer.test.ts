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

/**
 * Tripwires over the shared **explainer** (`components/shared/Explainer.tsx`).
 *
 * A source scan rather than a mount, for the same reason `overlay-contract.test.ts`
 * is one: this project runs Vitest with no jsdom (see `src/test-setup.ts`, a
 * minimal `document` stub), so a hook-bearing component cannot be rendered into
 * a container here. The properties below are all structural, and the behaviour
 * they stand for is exercised for real in
 * `e2e/thread-filter-explainer.spec.ts` (open, Escape, outside click).
 */
describe('Explainer: the trigger button', () => {
  it('is a non-submitting button', () => {
    // Explainers sit inside real forms (the trigger editor, the credential
    // modal), where the default `submit` type would save the form on a click.
    expect(source).toMatch(/<button[\s\S]*?type="button"[\s\S]*?class="icon-btn explainer-btn"/);
  });

  it('has an accessible name derived from the title', () => {
    // An icon-only button has no text, so aria-label is its only accessible
    // name (.claude/rules/frontend-css.md).
    expect(source).toMatch(/aria-label=\{`About \$\{title\}`\}/);
  });

  it('uses no native tooltip', () => {
    // `data-tooltip` is desktop-only by rule, so it cannot carry an explanation
    // to a phone; a native `title` attribute is banned outright. Scoped to the
    // trigger button's own attribute list, because `title` is also this
    // component's prop name and a bare /title=/ scan just matches that.
    expect(source).not.toMatch(/data-tooltip=/);
    const buttonAttrs = source.match(/<button\b([\s\S]*?)>\s*\n\s*<InfoIcon/);
    expect(buttonAttrs, 'found the trigger button').not.toBeNull();
    expect(buttonAttrs![1]).not.toMatch(/\stitle=/);
  });
});

describe('Explainer: delegates the dismiss contract', () => {
  it('builds its overlay through <Overlay>, passing the icon button as anchor', () => {
    // .claude/rules/frontend.md section "Modals & Popovers": every overlay goes
    // through the central component, and a toggle-opened one passes its toggle
    // as the anchor. With `anchor={null}` the outside-pointerdown dismiss races
    // the toggle and a touch reopens the dialog on the tap that dismissed it.
    expect(source).toMatch(/<Overlay\b/);
    expect(source).toMatch(/anchor=\{anchor\}/);
  });

  it('never hand-rolls its own dismiss listener', () => {
    expect(source).not.toMatch(/addEventListener\(\s*['"](?:pointerdown|mousedown|click)['"]/);
  });
});

describe('Explainer: the dialog escapes its host markup', () => {
  it('portals the dialog to <body>', () => {
    // An explainer is placed wherever its control is, which includes inside a
    // wrapping <label> (every checkbox row). A label forwards activation to its
    // control for clicks on any NON-interactive descendant, and a dialog's
    // paragraphs are not interactive content, so an inline dialog would toggle
    // the checkbox behind it every time the user tapped the explanation. The
    // button itself is exempt (it IS interactive content), which is exactly why
    // the hazard stays invisible until someone taps the copy.
    expect(source).toMatch(/createPortal\(dialog, document\.body\)/);
  });
});

describe('Explainer: host-only, never served to app iframes', () => {
  it('styles the explainer in host-components.css', () => {
    expect(hostCss).toMatch(/\.explainer-btn\b/);
    expect(hostCss).toMatch(/\.explainer-dialog\b/);
    expect(hostCss).toMatch(/\.explainer-body\b/);
  });

  it('keeps the classes out of the stylesheet the engine serves to apps', () => {
    // shared-components.css is include_str!'d into /api/v1/sdk-iframe.css, so a
    // class added there ships to every app iframe. The explainer is built on
    // <Overlay>, whose machinery no iframe has, so advertising it would offer
    // apps a component they cannot build.
    expect(sharedCss).not.toMatch(/explainer/);
  });
});

describe('Explainer: the icon never wraps away from its label', () => {
  it('glues the button to the label with a word joiner', () => {
    // A button is an atomic inline, and line breaking allows a break in front
    // of one. A squeezed label therefore drops the glyph onto a line of its
    // own, under the text. U+2060 WORD JOINER forbids that break.
    expect(source).toMatch(/<span class="explainer-slot">\s*<button/);
    expect(hostCss).toMatch(/\.explainer-slot::before \{\s*content:\s*'\\2060';/);
  });

  it('generates the joiner, so it stays out of the text and the accname', () => {
    // A text node would land in `textContent`, in the accessible name of the
    // label wrapping a checkbox row, and in a copied selection. It is a layout
    // instruction, so only the line breaker should ever see it.
    expect(source).not.toMatch(/\\u2060/);
  });

  it('keeps the slot a plain inline box, which is what exposes the joiner', () => {
    // An inline-flex or inline-block slot is an atomic inline itself: the outer
    // line breaker then sees one object and never reads the joiner, so the
    // orphan comes straight back. This is the tidy-up that would undo the fix.
    const rule = hostCss.match(/\.explainer-slot \{[\s\S]*?\n\}/);
    expect(rule, 'found the .explainer-slot rule').not.toBeNull();
    expect(rule![0]).toMatch(/display:\s*inline;/);
    expect(rule![0]).toMatch(/line-height:\s*0;/);
  });

  it('puts the space before the icon on the slot, where a gap row zeroes it', () => {
    // Wherever a row separates its children with a `gap`, the slot is the flex
    // item. So the margin lives there, or the two stack up again.
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
    // `middle` is defined as "baseline plus half the parent's x-height", so on
    // its own it parks a glyph that is taller than the text a good half a
    // lowercase letter low: the circle hangs into the descender space while its
    // top only just reaches the cap (reported on the Appearance page, where the
    // icon follows "Push notifications" and "Open links in"). The paired
    // negative `top` is what raises it to the cap-height centre, and it is in
    // `em` so it tracks whatever size label it rides.
    const rule = hostCss.match(/\.explainer-btn \{[\s\S]*?\n\}/);
    expect(rule, 'found the .explainer-btn rule').not.toBeNull();
    expect(rule![0]).toMatch(/vertical-align:\s*middle/);
    expect(rule![0]).toMatch(/\n\s*top:\s*-0?\.\d+em;/);
  });
});

describe('Explainer: the dialog is a named, focus-trapping modal', () => {
  it('names the panel, so a screen reader does not meet an unnamed dialog', () => {
    // The visible <h2> is not an accessible name. aria-label rather than
    // aria-labelledby, which would need an `id`.
    expect(source).toMatch(/panelProps=\{\{\s*'aria-label':\s*title\s*\}\}/);
  });

  it('traps Tab inside the panel and restores focus to the icon on close', () => {
    // A panel declaring aria-modal must not let the keyboard walk out into the
    // UI behind it. Same two shared helpers as ConfirmDialog, not a re-derived
    // trap.
    expect(source).toMatch(/trapDialogTab\(e, panelRef\.current\)/);
    expect(source).toMatch(/dialogOwnsKey\(e\.target, panelRef\.current\)/);
    expect(source).toMatch(/opener\?\.focus\(\)/);
  });
});

describe("FieldLabel: the button never becomes the label's own control", () => {
  it('renders the explainer OUTSIDE the <label>', () => {
    // A <button> is a labelable element, so inside a <label> whose control is a
    // sibling it becomes that label's control and clicking the label TEXT opens
    // the dialog. Guarded by construction: FieldLabel closes the label before
    // the Explainer.
    expect(source).toMatch(/<label>\{label \?\? title\}<\/label>\s*\n\s*\{children \? <Explainer/);
  });

  it('is what the sibling-control forms use, so no bare <label> wraps an Explainer', () => {
    for (const [name, src] of [
      ['TriggerDetails', triggerDetails],
      ['CredentialModal', credentialModal],
    ] as const) {
      // A `<label>` opening whose matching close comes AFTER an `<Explainer`
      // is the bug this guards. Cheap proxy: no `<Explainer` may appear between
      // a `<label>` and its `</label>` on the sibling-control forms.
      const between = /<label>(?:(?!<\/label>)[\s\S])*?<Explainer/.test(src);
      expect(between, `${name} puts an Explainer inside a <label>`).toBe(false);
    }
  });

  it('lets a checkbox row keep its explainer inline, where the input wins', () => {
    // The wrapping-label case is safe and deliberately untouched: the checkbox
    // is the FIRST labelable descendant, so it stays the label's control.
    // The class is matched loosely because that row carries a conditional dim
    // (`thread-filter-option-dimmed`) under a status view; what this pins is the
    // ORDER inside the label, not the class string.
    expect(filterPanel).toMatch(
      /<label class=[^>]*thread-filter-option[^>]*>[\s\S]*?<input[\s\S]*?<Explainer title="Include deleted">/,
    );
  });
});

describe('Explainer: sections of the same shape place the icon the same way', () => {
  /**
   * The category error, per the placement rule on `<Explainer>`: the icon goes
   * at the scope of what it explains, so two sections of the SAME shape must
   * not answer it differently. Appearance & Behavior had exactly that.
   * Notifications and Links are both one-control sections, and the icon sat on
   * the Notifications HEADING while Links carried its own on the row below, so
   * one page showed what read as two unrelated affordances.
   *
   * Scoped to that page's one-control sections rather than asserting "no
   * heading ever takes an icon", which would be false: a list section
   * (Repositories, Connect URLs) has no label narrower than its heading, and
   * Chat & triggers' copy covers two rows at once.
   */
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
    // The other half of the pair. If a later change hoists a row's copy up to
    // this heading, the page is mismatched again in the opposite direction.
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
