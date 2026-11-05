import { describe, expect, it } from 'vitest';
import { modelChoices, resolveSelection, type RegistryModel } from './_modelRegistry';

const route = (provider: string, configured: boolean | null, callable = true) => ({
  provider, id: `${provider}/wire`, configured,
  reasoning_efforts: provider === 'direct' ? ['low'] : ['high'],
  proxy: callable ? { name: provider, protocol: 'anthropic-messages' as const } : null,
});
const model = (id: string, ladder = false): RegistryModel => ({
  id, label: id, on_chat_ladder: ladder, successor: null, preferred_provider: 'unsupported',
  default_effort: null, routes: [route('unsupported', true, false), route('direct', null), route('other', true)],
});

describe('app model registry adapter', () => {
  it('groups ladder first, preserves registry order, and excludes retired rows', () => {
    expect(modelChoices([model('a'), model('b', true), { ...model('retired'), successor: 'b' }, model('c')])
      .map(row => row.value)).toEqual(['b', 'a', 'c']);
  });
  it('filters routes before resolving defaults and keeps unknown configuration visibly unset', () => {
    const [choice] = modelChoices([model('a')], { callableOnly: true });
    expect(choice.defaultProvider).toBe('other');
    expect(choice.providers?.map(p => [p.value, p.configured, p.reasoningEfforts]))
      .toEqual([['direct', false, ['low']], ['other', true, ['high']]]);
    expect(modelChoices([model('a')], { providers: ['missing'] })).toEqual([]);
  });
  it('returns the route actually selected, clamps efforts by route, and preserves null', () => {
    const choices = modelChoices([model('a')], { callableOnly: true });
    expect(resolveSelection(choices, { model: 'a', reasoning_effort: 'high', provider: 'direct' }))
      .toEqual({ model: 'a', reasoning_effort: 'low', provider: 'direct' });
    expect(resolveSelection(choices, { model: 'a', reasoning_effort: null, provider: null }))
      .toEqual({ model: 'a', reasoning_effort: null, provider: 'other' });
  });
  it('rejects unavailable explicit routes and model IDs instead of silently falling back', () => {
    const choices = modelChoices([model('a')], { callableOnly: true });
    expect(() => resolveSelection(choices, { model: 'a', reasoning_effort: null, provider: 'unsupported' })).toThrow();
    expect(() => resolveSelection(choices, { model: 'absent', reasoning_effort: null, provider: null })).toThrow();
  });
});
