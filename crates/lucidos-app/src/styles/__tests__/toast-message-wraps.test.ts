/**
 * A toast message wraps a long path; it never runs off the card.
 *
 * The regression: the stranded-frontend-Apply warning names the served `dist`
 * directory, so its message carries an absolute path. `.toast-body` set no
 * `overflow-wrap`, and the default breaks a long word only at its hyphens. The
 * leading `/Users/.../crates/lucidos-` was wider than the toast.
 *
 * Plain overflow would have been the mild version. `.toast-heading` scrolls
 * vertically, so it is a scroll box on both axes. The card grew a horizontal
 * scrollbar under the message and clipped a dozen characters out of the middle
 * of the path.
 *
 * Scanned rather than measured, for the same reason as the height cap beside
 * it: the failure is a property of the rule. Reproducing it needs a long
 * unbreakable token and a card narrow enough to defeat it.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');
const mobileCss = readFileSync(resolve(here, '../mobile.css'), 'utf-8');

/** The two values that let a token too wide for the card break inside itself.
 *  Either fixes this bug, since the toast has a definite width and the
 *  min-content difference between them cannot reach it. */
const BREAKING = ['anywhere', 'break-word'];

/** Anything in the message subtree, the linkified anchors inside it included.
 *  A plain selector match rather than `rulesTargeting`, because the subject
 *  rule here is the one that would break the chain: `.toast-heading a` styles
 *  the anchor, and its subject is the anchor, not the box. */
const MESSAGE_SUBTREE_RE =
  /\.toast-(body|heading|sections|section-title|section|bullets)\b/;

describe('a toast message wraps a long path', () => {
  it('breaks an over-wide token in the message column', () => {
    // On the column rather than on each box inside it: `overflow-wrap`
    // inherits, so one declaration covers the heading, the section titles, the
    // bullets and the linkified anchors in all three.
    const wrap = decl(block(componentsCss, '.toast-body {'), 'overflow-wrap');
    expect(BREAKING, `.toast-body sets overflow-wrap: ${wrap}`).toContain(wrap);
  });

  it('lets nothing in the message subtree opt back out of wrapping', () => {
    const offenders = [componentsCss, mobileCss].flatMap((sheet) =>
      cssRules(sheet)
        .filter((rule) => MESSAGE_SUBTREE_RE.test(rule.selector))
        .filter((rule) => rule.props.get('overflow-wrap') === 'normal'),
    );

    expect(
      offenders.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
      'a rule stops the message wrapping, so a long path overflows again',
    ).toEqual([]);
  });
});
