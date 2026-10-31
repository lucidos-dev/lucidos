/** A context section with no body says why. It offers the Capture context
 *  switch only when turning it on would record that body on later steps. */
import { describe, expect, it } from 'vitest';
import type { ContextSection } from '../../../store/types';
import { missingBodyReason } from '../missingBodyReason';

const bare = (name: string): ContextSection => ({ name, budget_delta_chars: 10 });
const withBody = (name: string): ContextSection => ({ ...bare(name), content: 'body' });

describe('missingBodyReason', () => {
  it('blames the switch when no section of a main-LLM step has a body', () => {
    const capture = { producer: 'main_llm' as const, sections: [bare('System Instructions'), bare('Tool Definitions (66)')] };
    expect(missingBodyReason(capture, false)).toBe('capture-off');
  });

  it('says the switch was off at the time when it is on now', () => {
    const capture = { producer: 'main_llm' as const, sections: [bare('System Instructions')] };
    expect(missingBodyReason(capture, true)).toBe('capture-off-at-step');
  });

  it('calls the section never recorded when a sibling has a body', () => {
    const capture = { producer: 'main_llm' as const, sections: [withBody('System Instructions'), bare('Tool Definitions (66)')] };
    expect(missingBodyReason(capture, false)).toBe('section-never-recorded');
    expect(missingBodyReason(capture, true)).toBe('section-never-recorded');
  });

  it('calls an auxiliary call never recorded, whatever the switch says', () => {
    const capture = { producer: 'auxiliary' as const, sections: [bare('Title')] };
    expect(missingBodyReason(capture, false)).toBe('section-never-recorded');
  });
});
