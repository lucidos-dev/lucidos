import { describe, it, expect } from 'vitest';
import {
  getCollapsedVisibleEvents,
  splitEventSections,
  mergeAdjacentTextEvents,
} from '../event-rendering';
import type { ResponseEvent } from '../types';

// ===========================================================================
// getCollapsedVisibleEvents
// ===========================================================================
describe('getCollapsedVisibleEvents', () => {
  it('shows only events from last text block onwards', () => {
    const events: ResponseEvent[] = [
      { type: 'text', md: 'early text' },
      { type: 'step', description: 'search', outcome: 'success' },
      { type: 'text', md: 'final answer' },
    ];
    const { visibleEvents, needsFallback } = getCollapsedVisibleEvents(events);
    expect(visibleEvents).toHaveLength(1);
    expect((visibleEvents[0] as { md: string }).md).toBe('final answer');
    expect(needsFallback).toBe(false);
  });

  it('returns all events if no text blocks', () => {
    const events: ResponseEvent[] = [
      { type: 'step', description: 'search', outcome: 'success' },
    ];
    const { visibleEvents, needsFallback } = getCollapsedVisibleEvents(events);
    expect(visibleEvents).toHaveLength(1);
    expect(needsFallback).toBe(true);
  });

  it('handles empty events', () => {
    const { visibleEvents, needsFallback } = getCollapsedVisibleEvents([]);
    expect(visibleEvents).toHaveLength(0);
    expect(needsFallback).toBe(true);
  });

  it('preserves image events even when they appear before the last text block', () => {
    const events: ResponseEvent[] = [
      { type: 'step', description: 'Thinking', outcome: 'success' },
      { type: 'step', description: 'generate_image', outcome: 'success' },
      { type: 'image', base64: 'abc123', mime_type: 'image/jpeg', prompt: 'a cowboy on a reindeer' },
      { type: 'text', md: 'Here is the generated image!' },
    ];
    const { visibleEvents } = getCollapsedVisibleEvents(events);
    const images = visibleEvents.filter(e => e.type === 'image');
    expect(images).toHaveLength(1);
    expect((images[0] as { base64: string }).base64).toBe('abc123');
  });

  it('preserves section_break events even when they appear before the last text block', () => {
    const events: ResponseEvent[] = [
      { type: 'text', md: 'Engine response' },
      { type: 'section_break', channel: 'claude_code' },
      { type: 'step', description: 'Edit file.rs', outcome: 'success' },
      { type: 'text', md: 'CC output' },
    ];
    const { visibleEvents } = getCollapsedVisibleEvents(events);
    const breaks = visibleEvents.filter(e => e.type === 'section_break');
    expect(breaks).toHaveLength(1);
  });
});

// ===========================================================================
// splitEventSections
// ===========================================================================
describe('splitEventSections', () => {
  it('splits at section_break boundaries', () => {
    const events: ResponseEvent[] = [
      { type: 'text', md: 'main text' },
      { type: 'section_break', channel: 'claude_code' },
      { type: 'text', md: 'cc text' },
    ];
    const sections = splitEventSections(events);
    expect(sections).toHaveLength(2);
    expect(sections[0]).toHaveLength(1);
    expect(sections[1]).toHaveLength(1);
  });

  it('handles no section breaks', () => {
    const events: ResponseEvent[] = [
      { type: 'text', md: 'just text' },
    ];
    const sections = splitEventSections(events);
    expect(sections).toHaveLength(1);
  });

  it('handles empty events', () => {
    expect(splitEventSections([])).toHaveLength(0);
  });
});

// ===========================================================================
// mergeAdjacentTextEvents
// ===========================================================================
describe('mergeAdjacentTextEvents', () => {
  it('merges consecutive text events into one', () => {
    const events: ResponseEvent[] = [
      { type: 'text', md: 'Hello ' },
      { type: 'text', md: 'world' },
    ];
    const merged = mergeAdjacentTextEvents(events);
    expect(merged).toHaveLength(1);
    expect((merged[0] as { md: string }).md).toBe('Hello world');
  });

  it('preserves non-text events between text runs', () => {
    const events: ResponseEvent[] = [
      { type: 'text', md: 'before ' },
      { type: 'text', md: 'tool' },
      { type: 'step', description: 'read_file', outcome: 'success' },
      { type: 'text', md: 'after tool' },
    ];
    const merged = mergeAdjacentTextEvents(events);
    expect(merged).toHaveLength(3);
    expect(merged[0].type).toBe('text');
    expect((merged[0] as { md: string }).md).toBe('before tool');
    expect(merged[1].type).toBe('step');
    expect(merged[2].type).toBe('text');
    expect((merged[2] as { md: string }).md).toBe('after tool');
  });

  it('handles empty input', () => {
    expect(mergeAdjacentTextEvents([])).toHaveLength(0);
  });

  it('preserves code blocks split across text events', () => {
    const events: ResponseEvent[] = [
      { type: 'text', md: 'Here is code:\n```tsx\nfunction foo() {\n' },
      { type: 'text', md: '  return 1;\n}\n```\nDone.' },
    ];
    const merged = mergeAdjacentTextEvents(events);
    expect(merged).toHaveLength(1);
    expect((merged[0] as { md: string }).md).toContain('```tsx');
    expect((merged[0] as { md: string }).md).toContain('```\nDone.');
  });

  it('returns single text event unchanged', () => {
    const events: ResponseEvent[] = [
      { type: 'text', md: 'solo' },
    ];
    const merged = mergeAdjacentTextEvents(events);
    expect(merged).toHaveLength(1);
    expect((merged[0] as { md: string }).md).toBe('solo');
  });

  it('handles only non-text events', () => {
    const events: ResponseEvent[] = [
      { type: 'step', description: 'search', outcome: 'success' },
      { type: 'image', base64: 'abc', mime_type: 'image/jpeg' },
    ];
    const merged = mergeAdjacentTextEvents(events);
    expect(merged).toHaveLength(2);
  });
});
