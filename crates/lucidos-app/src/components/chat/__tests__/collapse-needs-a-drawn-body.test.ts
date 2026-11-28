import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

describe('a turn is collapsible only while its body draws something', () => {
  it('gates canCollapse on what is drawn, not on the event count', () => {
    expect(source).toMatch(
      /const canCollapse = hasResponse \|\| events\.some\(\(e\) => drawsResponseRow\(e, showSteps\)\)/,
    );
    expect(source).not.toMatch(/const canCollapse = hasResponse \|\| hasEvents/);
  });

  it('holds the fold to the same gate, so a blank turn cannot render as folded', () => {
    expect(source).toMatch(/const isCollapsed = canCollapse && collapsedExchanges\.value\.has\(/);
  });

  it('asks the panel the same question, rather than spelling it a second way', () => {
    expect(source).toMatch(/hasBody=\{canCollapse\}/);
    expect(source).not.toMatch(/hasBody=\{hasResponse \|\| hasEvents\}/);
  });
});

describe('the initiator fold skips every chipless turn', () => {
  it('gates it on the chip test, beside the body test', () => {
    expect(source).toMatch(
      /const canCollapseInitiator = !isChromeless\s*\n?\s*&& \(!!initiator\.summary \|\| !!initiator\.details\)/,
    );
    expect(source).toMatch(
      /const isChromeless = isUserMessageBubble \|\| isChangePanel \|\| isSpeechOnly/,
    );
  });

  for (const predicate of ['isChangePanel', 'isUserMessageBubble', 'isSpeechOnly', 'isChromeless']) {
    it(`reads ${predicate} from above, so the gate cannot see it undefined`, () => {
      expect(source.indexOf(`const ${predicate} =`)).toBeLessThan(
        source.indexOf('const canCollapseInitiator ='),
      );
    });
  }
});

describe('turning a reveal on lifts this turn\'s fold', () => {
  it('expands on the way ON only, and never folds on the way off', () => {
    const fn = source.match(/function reveal\(setting: Signal<boolean>\)[\s\S]*?\n  \}/);
    expect(fn, '`reveal` not found').not.toBeNull();
    const body = fn![0];
    expect(body).toMatch(/setting\.value = !setting\.value/);
    expect(body).toMatch(/if \(setting\.value\) expandExchange\(threadId, exchange\.userSeq\)/);
    expect(body).not.toMatch(/toggleExchangeCollapsed/);
  });

  it('routes both reveals through it, so the two cannot drift', () => {
    expect(source).toMatch(/const toggleDetails = heldOnThePress\(\(\) => reveal\(detailsExpanded\)\)/);
    expect(source).toMatch(/const toggleSteps = heldOnThePress\(\(\) => reveal\(stepsExpanded\)\)/);
  });

  it('keeps the scroll anchor around the whole thing', () => {
    const fn = source.match(/function heldOnThePress[\s\S]*?\n\}/);
    expect(fn, '`heldOnThePress` not found').not.toBeNull();
    expect(fn![0]).toMatch(/withScrollAnchor\(e\.currentTarget as HTMLElement \| null, fn\)/);
  });

  it('anchors every control that changes a turn\'s height', () => {
    for (const held of [
      'const toggleDetails = heldOnThePress(',
      'const toggleSteps = heldOnThePress(',
      'const toggleCollapsed = heldOnThePress(',
      'const toggleInitiator = heldOnThePress(',
    ]) {
      expect(source, `${held} is not anchored`).toContain(held);
    }
  });
});
