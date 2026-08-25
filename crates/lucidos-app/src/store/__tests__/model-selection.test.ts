import { describe, expect, it } from 'vitest';
import {
  clampToOffered,
  decodePair,
  filterModelRows,
  encodePair,
  formatPair,
  modelRows,
  pairLabelOf,
  tiersOf,
  lucidosTiers,
  tierOptions,
  type ModelChoice,
  type TierChoice,
} from '../modelSelection';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';

/** The Lucidos Agent's vocabulary, as `store/models.ts` spells it. */
const LUCIDOS: TierChoice[] = [
  { value: 'none', label: 'Off' },
  { value: 'low', label: 'Low' },
  { value: 'medium', label: 'Med' },
  { value: 'high', label: 'High' },
  { value: 'xhigh', label: 'X-High' },
  { value: 'max', label: 'Max' },
];

/** A coding agent's vocabulary: different labels, no `none`, and descriptions. */
const CODING_AGENT: TierChoice[] = [
  { value: 'low', label: 'Low', description: 'Minimal thinking' },
  { value: 'medium', label: 'Medium', description: 'Balanced' },
  { value: 'high', label: 'High', description: 'Deep' },
  { value: 'xhigh', label: 'Extra High', description: 'Deeper' },
  { value: 'max', label: 'Max', description: 'Maximum' },
];

/** The Codex model rows as the engine now serves them: the matrix transposed
 *  out of each effort's `supported_models`. */
const CODEX_MODELS: ModelChoice[] = [
  { value: 'default', label: 'Default', reasoningEfforts: ['low', 'medium', 'high', 'xhigh'] },
  {
    value: 'gpt-5.6-sol',
    label: 'GPT-5.6 Sol',
    reasoningEfforts: ['low', 'medium', 'high', 'xhigh', 'max'],
  },
  { value: 'gpt-5.5', label: 'GPT-5.5', reasoningEfforts: ['low', 'medium', 'high', 'xhigh'] },
];

describe('tierOptions', () => {
  it('narrows a surface vocabulary to what the model accepts', () => {
    expect(tierOptions(['low', 'high'], LUCIDOS).map((t) => t.value)).toEqual(['low', 'high']);
  });

  it('keeps the surface labels rather than deriving them', () => {
    expect(tierOptions(['medium'], LUCIDOS)[0].label).toBe('Med');
    expect(tierOptions(['medium'], CODING_AGENT)[0].label).toBe('Medium');
  });

  it('is empty for a model with no tiers, which renders no effort control', () => {
    expect(tierOptions([], LUCIDOS)).toEqual([]);
  });
});

describe('clampToOffered', () => {
  it('leaves a supported tier alone', () => {
    expect(clampToOffered('high', LUCIDOS)).toBe('high');
  });

  it('breaks a tie toward the higher tier', () => {
    // `xhigh` sits one rung from both `high` and `max` on the Claude budget
    // path, and reaching past `high` must not be answered with less.
    const budgetPath = tierOptions(['none', 'low', 'medium', 'high', 'max'], LUCIDOS);
    expect(clampToOffered('xhigh', budgetPath)).toBe('max');
  });

  it('takes the genuinely nearest tier even when that is lower', () => {
    const gemini = tierOptions(['none', 'low', 'medium', 'high'], LUCIDOS);
    expect(clampToOffered('max', gemini)).toBe('high');
  });

  it('caps a coding-agent model that does not accept max', () => {
    expect(clampToOffered('max', tierOptions(CODEX_MODELS[2].reasoningEfforts, CODING_AGENT)))
      .toBe('xhigh');
  });

  it('answers nothing when nothing is selected yet', () => {
    // A coding-agent menu with no session and no pick. Answering would claim a
    // tier the request will not carry.
    expect(clampToOffered(null, CODING_AGENT)).toBeNull();
  });

  it('answers nothing when the model has no tiers', () => {
    expect(clampToOffered('high', [])).toBeNull();
  });

  it('takes the top offered tier for a value off the ladder', () => {
    // A display choice, not a billing one: the wire re-checks it.
    expect(clampToOffered('ultra', tierOptions(['low', 'medium'], LUCIDOS))).toBe('medium');
  });
});

describe('tiersOf', () => {
  it('reads the tiers off the served model row', () => {
    expect(tiersOf(CODEX_MODELS, 'gpt-5.6-sol')).toContain('max');
    expect(tiersOf(CODEX_MODELS, 'gpt-5.5')).not.toContain('max');
  });

  it('offers nothing for a model with no row', () => {
    // An effort a model rejects fails the whole turn, so a guess is worse than
    // no control.
    expect(tiersOf(CODEX_MODELS, 'gpt-9')).toEqual([]);
    expect(tiersOf(CODEX_MODELS, null)).toEqual([]);
  });
});

describe('lucidosTiers', () => {
  it('uses the registry answer when there is one', () => {
    expect(lucidosTiers('muse-glimmer:30b-mlx', ['none', 'low', 'medium', 'high']))
      .toEqual(['none', 'low', 'medium', 'high']);
  });

  it('falls back to the id-shape heuristic before the registry loads', () => {
    expect(lucidosTiers('gpt-5.4')).not.toContain('max');
    expect(lucidosTiers('claude-opus-5@default')).toContain('xhigh');
  });
});

/** The hook is a pure function of its input, so it is exercised directly
 *  rather than through a rendered component. */
function selectionFor(models: ModelChoice[], vocabulary: TierChoice[], model: string | null, effort: string | null) {
  const patches: ModelSelectionPatch[] = [];
  const selection = useModelSelection({
    models,
    vocabulary,
    model,
    effort,
    onChange: (p) => patches.push(p),
  });
  return { selection, patches };
}

describe('modelRows', () => {
  it('offers one row per model, carrying the tiers it accepts', () => {
    const rows = modelRows([CODEX_MODELS[2]], CODING_AGENT);
    expect(rows.map((r) => r.value)).toEqual(['gpt-5.5']);
    expect(rows[0].tiers.map((t) => t.value)).toEqual(['low', 'medium', 'high', 'xhigh']);
  });

  it('offers no tier the model rejects', () => {
    // `RoutingProvider` would clamp it behind the user's back.
    const rows = modelRows([CODEX_MODELS[2]], CODING_AGENT);
    expect(rows[0].tiers.some((t) => t.value === 'max')).toBe(false);
  });

  it('leaves a model with no tiers with no second step', () => {
    const rows = modelRows(
      [{ value: 'imagen-4', label: 'Imagen 4', reasoningEfforts: [] }],
      LUCIDOS,
    );
    expect(rows).toEqual([{
      value: 'imagen-4', label: 'Imagen 4', description: undefined, tiers: [],
    }]);
  });

  it('keeps the model description for the row that shows it', () => {
    const rows = modelRows(
      [{ value: 'gpt-5.5', label: 'GPT-5.5', description: 'Fast', reasoningEfforts: ['low'] }],
      CODING_AGENT,
    );
    expect(rows[0].description).toBe('Fast');
  });
});

describe('filterModelRows', () => {
  const ROWS = modelRows(CODEX_MODELS, CODING_AGENT);

  it('returns everything for an empty query', () => {
    expect(filterModelRows(ROWS, '   ')).toEqual([...ROWS]);
  });

  it('narrows to one model by its name', () => {
    expect(filterModelRows(ROWS, 'sol').map((r) => r.value)).toEqual(['gpt-5.6-sol']);
  });

  it('takes every term, so two words narrow together', () => {
    expect(filterModelRows(ROWS, 'gpt 5.6').map((r) => r.value)).toEqual(['gpt-5.6-sol']);
  });

  it('never matches a tier, which is the NEXT step', () => {
    // Every model offers much the same handful, so matching them here would
    // return the whole list for any tier name.
    expect(filterModelRows(ROWS, 'max')).toEqual([]);
  });

  it('keeps a tierless model, which is a whole selection on its own', () => {
    const rows = modelRows(
      [{ value: 'imagen-4', label: 'Imagen 4', reasoningEfforts: [] }],
      LUCIDOS,
    );
    expect(filterModelRows(rows, 'imagen').map((r) => r.value)).toEqual(['imagen-4']);
  });

  it('answers nothing when nothing matches', () => {
    expect(filterModelRows(ROWS, 'nonesuch')).toEqual([]);
  });
});

describe('pairLabelOf', () => {
  const ROWS = modelRows(CODEX_MODELS, CODING_AGENT);

  it('reads an encoded pair as both halves', () => {
    expect(pairLabelOf(ROWS, 'gpt-5.5|xhigh')).toBe('GPT-5.5 \u00b7 Extra High');
  });

  it('reads a tierless selection as the model alone', () => {
    const rows = modelRows(
      [{ value: 'imagen-4', label: 'Imagen 4', reasoningEfforts: [] }],
      LUCIDOS,
    );
    expect(pairLabelOf(rows, 'imagen-4|')).toBe('Imagen 4');
  });

  it('falls back to the raw halves for a model the rows no longer list', () => {
    expect(pairLabelOf(ROWS, 'gpt-9|high')).toBe('gpt-9 \u00b7 high');
  });
});

describe('encodePair / decodePair', () => {
  it('round-trips a pair', () => {
    expect(decodePair(encodePair('gpt-5.5', 'high'))).toEqual({ model: 'gpt-5.5', effort: 'high' });
  });

  it('round-trips a model id that carries the separator', () => {
    // Splitting on the FIRST separator would select a different model.
    const odd = 'weird|model';
    expect(decodePair(encodePair(odd, 'low'))).toEqual({ model: odd, effort: 'low' });
  });

  it('reads a tierless model as a whole selection', () => {
    expect(decodePair(encodePair('imagen-4', null))).toEqual({ model: 'imagen-4', effort: null });
  });

  it('round-trips a TIERLESS id that carries the separator', () => {
    // The registry is user-extensible, so no id shape can be promised. Without
    // the trailing separator this read back as `weird` at tier `model`.
    expect(decodePair(encodePair('weird|model', null)))
      .toEqual({ model: 'weird|model', effort: null });
  });

  it('never encodes a tierless model onto a real pair', () => {
    // `weird|model` with no tiers and `weird` at tier `model` are two different
    // selections, and both can be rows in one list.
    expect(encodePair('weird|model', null)).not.toBe(encodePair('weird', 'model'));
  });

  it('reads a value with no separator as a bare model', () => {
    expect(decodePair('imagen-4')).toEqual({ model: 'imagen-4', effort: null });
  });
});

describe('formatPair', () => {
  it('joins the two halves', () => {
    expect(formatPair('Opus 5 (1M)', 'X-High')).toBe('Opus 5 (1M) \u00b7 X-High');
  });

  it('leaves no trailing separator when the model has no tiers', () => {
    expect(formatPair('Imagen 4', null)).toBe('Imagen 4');
  });
});

describe('useModelSelection', () => {
  it('reports both halves in one patch, so nothing can be half-applied', () => {
    const { selection, patches } = selectionFor(CODEX_MODELS, CODING_AGENT, 'gpt-5.6-sol', 'max');
    selection.pick('gpt-5.5|xhigh');
    expect(patches).toEqual([{ model: 'gpt-5.5', reasoningEffort: 'xhigh' }]);
  });

  it('reports a tierless model with no effort at all', () => {
    const imageModels: ModelChoice[] = [
      { value: 'imagen-4', label: 'Imagen 4', reasoningEfforts: [] },
      { value: 'gpt-image-2', label: 'GPT Image 2', reasoningEfforts: [] },
    ];
    const { selection, patches } = selectionFor(imageModels, LUCIDOS, 'imagen-4', null);
    selection.pick('gpt-image-2|');
    expect(patches).toEqual([{ model: 'gpt-image-2', reasoningEffort: null }]);
  });

  it('renders one row and no tiers when the model has none', () => {
    const imageModels: ModelChoice[] = [{ value: 'imagen-4', label: 'Imagen 4', reasoningEfforts: [] }];
    const { selection } = selectionFor(imageModels, LUCIDOS, 'imagen-4', 'high');
    expect(selection.rows.map((r) => r.value)).toEqual(['imagen-4']);
    expect(selection.rows[0].tiers).toEqual([]);
    expect(selection.effort).toBeNull();
    expect(selection.value).toBe('imagen-4|');
  });

  it('shows the clamp rather than the stored tier when the model dropped it', () => {
    // A pick cannot leave a stale half behind any more, but a preference
    // written before the pair became one thing still can.
    const { selection } = selectionFor(CODEX_MODELS, CODING_AGENT, 'gpt-5.5', 'max');
    expect(selection.effort).toBe('xhigh');
    expect(selection.value).toBe('gpt-5.5|xhigh');
    expect(selection.label).toBe('GPT-5.5 \u00b7 Extra High');
  });

  it('labels an unlisted model by its id rather than blank', () => {
    const { selection } = selectionFor(CODEX_MODELS, CODING_AGENT, 'gpt-9', null);
    expect(selection.label).toBe('gpt-9');
  });
});
