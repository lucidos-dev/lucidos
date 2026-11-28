import { describe, it, expect } from 'vitest';
import { contextCeilingOptions } from './CodingAgentContextCeilingSection';
import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';

const DEFAULT = PREFERENCE_CATALOG.coding_agent_context_ceiling.fallback;

describe('the context ceiling row', () => {
  it('offers the catalog default, marked as the default', () => {
    const options = contextCeilingOptions(DEFAULT);
    const fallback = options.find((o) => o.value === DEFAULT);
    expect(fallback?.description).toContain('Default');
    expect(options.map((o) => o.value)).toContain('off');
  });

  it('keeps a value set elsewhere among the options', () => {
    const options = contextCeilingOptions('250k');
    expect(options.map((o) => o.value)).toContain('250k');
    expect(contextCeilingOptions(DEFAULT).map((o) => o.value)).not.toContain('250k');
  });

  it('names each size the way the setting is written', () => {
    const labels = contextCeilingOptions(DEFAULT).map((o) => o.label);
    expect(labels).toContain('Off');
    expect(labels).toContain('300k');
    expect(labels).toContain('30% of the window');
  });
});
