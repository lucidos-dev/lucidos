import { describe, it, expect } from 'vitest';
import {
  STANDARD_ID,
  MAX_ID_CHARS,
  MAX_INSTRUCTION_CHARS,
  MAX_LABEL_CHARS,
  MAX_STYLES,
  describeStyleProblem,
  documentWithEdit,
  documentWithout,
  isValidStyleId,
  libraryIsFull,
  nextStyleId,
} from './responseStyle';
import { responseStyleOptions } from './ResponseStylesSection';
import type { ResponseStyle } from '../../api/types';

function style(over: Partial<ResponseStyle> & { id: string }): ResponseStyle {
  return {
    label: over.id,
    description: 'what it does',
    instruction: '- do a thing',
    source: 'user',
    editable: true,
    ...over,
  };
}

/** What the engine serves a workspace that has changed nothing. */
const SHIPPED: ResponseStyle[] = [
  style({
    id: STANDARD_ID,
    label: 'Standard',
    description: 'Nothing is added. Answers come back the way they do today.',
    instruction: '',
    source: 'builtin',
    editable: false,
  }),
  style({
    id: 'concise',
    label: 'Concise',
    description: 'Answer first, no preamble or recap. Usually a few sentences.',
    instruction: '- Lead with the answer.',
    source: 'builtin',
  }),
  style({
    id: 'minimal',
    label: 'Minimal',
    description: 'The short answer only, and no follow-up question.',
    instruction: '- Answer and stop.',
    source: 'builtin',
  }),
];

describe('the response style picker', () => {
  it('offers every style, with a one-line description each', () => {
    const options = responseStyleOptions(SHIPPED);
    expect(options.map((o) => o.value)).toEqual([STANDARD_ID, 'concise', 'minimal']);
    for (const option of options) {
      expect(option.label.length).toBeGreaterThan(0);
      // The description is what tells a user what the setting DOES to an
      // answer. A row without one is a name they have to guess at.
      expect(option.description?.length ?? 0).toBeGreaterThan(0);
    }
  });

  it('leads with Standard, the default and the way back', () => {
    expect(responseStyleOptions(SHIPPED)[0].value).toBe(STANDARD_ID);
  });

  it('offers a style the user added', () => {
    const library = [...SHIPPED, style({ id: 'board-report', label: 'Board report' })];
    expect(responseStyleOptions(library).map((o) => o.value)).toContain('board-report');
  });
});

describe('the stored document', () => {
  it('holds nothing at all while every shipped style is untouched', () => {
    // The promise behind Reset, and behind a later reword of the shipped text
    // reaching whoever left it alone: an untouched row is simply absent.
    expect(documentWithout(SHIPPED, 'nothing')).toEqual([]);
  });

  it('records an edited shipped style as an override', () => {
    const document = documentWithEdit(SHIPPED, {
      id: 'concise',
      label: 'Mine',
      instruction: '- My own take.',
    });
    expect(document).toEqual([
      { id: 'concise', label: 'Mine', instruction: '- My own take.' },
    ]);
  });

  it('keeps the OTHER saved rows when one is edited', () => {
    const library = [
      ...SHIPPED.slice(0, 1),
      style({ id: 'concise', label: 'Mine', instruction: '- Mine.', source: 'overridden' }),
      ...SHIPPED.slice(2),
      style({ id: 'board-report', label: 'Board report', instruction: '- Three bullets.' }),
    ];
    const document = documentWithEdit(library, {
      id: 'board-report',
      label: 'Board report',
      instruction: '- Two bullets.',
    });
    // The other override survives. Rebuilding the document from the library is
    // what guarantees that, where patching the old one could drop it.
    expect(document.map((e) => e.id).sort()).toEqual(['board-report', 'concise']);
    expect(document.find((e) => e.id === 'concise')?.instruction).toBe('- Mine.');
  });

  it('drops only the named row, which is both Reset and Delete', () => {
    const library = [
      ...SHIPPED.slice(0, 1),
      style({ id: 'concise', label: 'Mine', instruction: '- Mine.', source: 'overridden' }),
      ...SHIPPED.slice(2),
      style({ id: 'board-report', label: 'Board report' }),
    ];
    // Reset a shipped style: the override goes, the user style stays.
    expect(documentWithout(library, 'concise').map((e) => e.id)).toEqual(['board-report']);
    // Delete a user style: the override stays.
    expect(documentWithout(library, 'board-report').map((e) => e.id)).toEqual(['concise']);
  });

  it('never writes an entry for Standard', () => {
    // The engine refuses one outright. Sending it would turn a save into a
    // toast, so the client must not build one in the first place.
    const document = documentWithEdit(SHIPPED, {
      id: 'board-report',
      label: 'Board report',
      instruction: '- Three bullets.',
    });
    expect(document.some((e) => e.id === STANDARD_ID)).toBe(false);
    expect(documentWithout(SHIPPED, 'board-report').some((e) => e.id === STANDARD_ID)).toBe(
      false,
    );
  });

  it('trims the saved label and instruction', () => {
    const [entry] = documentWithEdit(SHIPPED, {
      id: 'mine',
      label: '  Mine  ',
      instruction: '  - Mine.\n',
    });
    expect(entry.label).toBe('Mine');
    expect(entry.instruction).toBe('- Mine.');
  });

  it('replaces an edited row IN PLACE, so the picker does not reshuffle', () => {
    // Document order is picker order for a user's own styles. Appending would
    // move a style to the bottom every time its typo was fixed.
    const library = [
      ...SHIPPED,
      style({ id: 'first', instruction: '- a' }),
      style({ id: 'second', instruction: '- b' }),
      style({ id: 'third', instruction: '- c' }),
    ];
    const document = documentWithEdit(library, {
      id: 'first',
      label: 'First',
      instruction: '- edited',
    });
    expect(document.map((e) => e.id)).toEqual(['first', 'second', 'third']);
    expect(document[0].instruction).toBe('- edited');
  });

  it('refuses an edit that would push the document past the limit', () => {
    // The Add card is already hidden here, but Edit is not: overriding an
    // untouched shipped style adds a row the engine would then refuse. The
    // bound is on the document, so the check has to be too.
    const full = [
      ...SHIPPED,
      ...Array.from({ length: MAX_STYLES }, (_, i) => style({ id: `style-${i}` })),
    ];
    const draft = { id: 'concise', label: 'Mine', instruction: '- Mine.' };
    const nextSize = documentWithEdit(full, draft).length;
    expect(nextSize).toBe(MAX_STYLES + 1);

    const taken = full.filter((s) => s.id !== 'concise').map((s) => s.id);
    expect(describeStyleProblem(draft, taken, nextSize)).toContain(String(MAX_STYLES));
    // Editing a style that ALREADY has a row does not grow the document, so a
    // full library must still allow it.
    const own = { id: 'style-0', label: 'Mine', instruction: '- Mine.' };
    const ownSize = documentWithEdit(full, own).length;
    expect(ownSize).toBe(MAX_STYLES);
    expect(describeStyleProblem(own, [], ownSize)).toBeNull();
  });

  it('withholds Add once the document is full', () => {
    const full = [
      ...SHIPPED,
      ...Array.from({ length: MAX_STYLES }, (_, i) => style({ id: `style-${i}` })),
    ];
    expect(libraryIsFull(full)).toBe(true);
    expect(libraryIsFull(SHIPPED)).toBe(false);
  });
});

describe('what the engine would refuse', () => {
  it('accepts a kebab-case id and refuses the rest', () => {
    for (const good of ['a', 'board-report', 'style-2']) {
      expect(isValidStyleId(good)).toBe(true);
    }
    for (const bad of ['', 'Board Report', 'board_report', '-lead', 'trail-', 'a--b']) {
      expect(isValidStyleId(bad)).toBe(false);
    }
    expect(isValidStyleId('x'.repeat(MAX_ID_CHARS))).toBe(true);
    expect(isValidStyleId('x'.repeat(MAX_ID_CHARS + 1))).toBe(false);
  });

  it('derives a usable id from a typed name', () => {
    expect(nextStyleId('Board report', [])).toBe('board-report');
    expect(nextStyleId('  Terse!!  ', [])).toBe('terse');
    // NFKD, so an accent keeps its letter. A bare fold gives `caf-report`,
    // which is not what the engine's `slugify_kebab` would store.
    expect(nextStyleId('Café report', [])).toBe('cafe-report');
    expect([...nextStyleId('x'.repeat(80), [])].length).toBeLessThanOrEqual(MAX_ID_CHARS);
  });

  it('gives a name the charset cannot hold a usable id anyway', () => {
    // Lucidos runs in whatever language `language` names. A Japanese or
    // Cyrillic name folds to nothing, and leaving Save dead over an id the
    // form never shows is not an answer.
    for (const label of ['要点だけ', 'Кратко', 'موجز', '###']) {
      const id = nextStyleId(label, []);
      expect(isValidStyleId(id)).toBe(true);
      expect(describeStyleProblem({ id, label, instruction: '- x' }, [])).toBeNull();
    }
  });

  it('never hands back an id the library already holds', () => {
    expect(nextStyleId('Board report', ['board-report'])).toBe('board-report-2');
    expect(nextStyleId('Board report', ['board-report', 'board-report-2']))
      .toBe('board-report-3');
    expect(nextStyleId('###', ['style-2'])).toBe('style-3');
  });

  it('names every bound the engine enforces, so Save can say why', () => {
    const ok = { id: 'mine', label: 'Mine', instruction: '- Mine.' };
    expect(describeStyleProblem(ok, [])).toBeNull();

    expect(describeStyleProblem({ ...ok, id: STANDARD_ID }, [])).toContain('off switch');
    expect(describeStyleProblem({ ...ok, id: 'Bad Id' }, [])).toContain(String(MAX_ID_CHARS));
    expect(describeStyleProblem(ok, ['mine'])).toContain('already exists');
    expect(describeStyleProblem({ ...ok, label: '  ' }, [])).toContain('name');
    expect(
      describeStyleProblem({ ...ok, label: 'x'.repeat(MAX_LABEL_CHARS + 1) }, []),
    ).toContain(String(MAX_LABEL_CHARS));
    expect(describeStyleProblem({ ...ok, instruction: '  ' }, [])).toContain('answer');
    expect(
      describeStyleProblem({ ...ok, instruction: 'x'.repeat(MAX_INSTRUCTION_CHARS + 1) }, []),
    ).toContain(String(MAX_INSTRUCTION_CHARS));
  });

  it('counts code points, matching the engine, not UTF-16 units', () => {
    // The engine counts Rust `chars`, which are code points. A bare `.length`
    // here counts UTF-16 units, so an astral character costs two and Save goes
    // dead on a paragraph the engine would happily take.
    const wide = '😀'.repeat(MAX_INSTRUCTION_CHARS);
    expect(wide.length).toBe(MAX_INSTRUCTION_CHARS * 2);
    expect(
      describeStyleProblem({ id: 'mine', label: 'Mine', instruction: wide }, []),
    ).toBeNull();

    const over = '😀'.repeat(MAX_INSTRUCTION_CHARS + 1);
    expect(
      describeStyleProblem({ id: 'mine', label: 'Mine', instruction: over }, []),
    ).toContain(String(MAX_INSTRUCTION_CHARS));
  });

  it('lets a shipped style keep its id while its name changes', () => {
    // Editing Concise must not be refused as a duplicate of itself. Nor may it
    // save under a new id, which would leave the original in place.
    const taken = SHIPPED.filter((s) => s.id !== 'concise').map((s) => s.id);
    expect(
      describeStyleProblem({ id: 'concise', label: 'Terse', instruction: '- Short.' }, taken),
    ).toBeNull();
  });
});
