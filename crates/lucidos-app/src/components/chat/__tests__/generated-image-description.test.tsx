import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { GeneratedImage, imagePromptSummary } from '../chat-exchange-parts';
import { exchangeResponseEvents, groupIntoExchanges, type ThreadEvent } from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';

const PROMPT = 'A humorous, photorealistic cowboy riding a reindeer through a snowy forest, high detail';

function renderedImage(toolCall: ThreadEvent): Extract<ResponseEvent, { type: 'image' }> | undefined {
  const events = new Map<number, ThreadEvent>([
    [1, { type: 'MessageReceived', text: 'make me a picture' }],
    [2, toolCall],
    [3, { type: 'ToolResult', name: 'generate_image', result: 'Image generated successfully.', images: ['b64bytes'] }],
    [4, { type: 'ResponseGenerated' }],
  ]);
  const [exchange] = groupIntoExchanges(events);
  return exchangeResponseEvents(exchange).find(e => e.type === 'image') as
    Extract<ResponseEvent, { type: 'image' }> | undefined;
}

/** The image carries the prompt it was generated from, because that is the only
 *  thing in the conversation that describes it. The `thread:N` reference it used
 *  to carry instead is an LLM-facing handle (see `view_image` / `input_images`
 *  in the engine's image tools) and says nothing to a human. */
describe('generated image description in the render layer', () => {
  it('carries the generating call\'s un-elided prompt', () => {
    const image = renderedImage({ type: 'ToolCalled', name: 'generate_image', args: { prompt: PROMPT } });
    expect(image?.prompt).toBe(PROMPT);
  });

  it('carries no prompt when the generating call recorded no args', () => {
    const image = renderedImage({ type: 'ToolCalled', name: 'generate_image', args: {} });
    expect(image).toBeDefined();
    expect(image?.prompt).toBeUndefined();
  });
});

describe('imagePromptSummary', () => {
  it('flattens the prompt to a single line', () => {
    expect(imagePromptSummary('a cowboy\n\n  on a   reindeer ')).toBe('a cowboy on a reindeer');
  });

  it('caps a long prompt with an ellipsis', () => {
    const summary = imagePromptSummary('x'.repeat(500), 10);
    expect(summary).toBe('xxxxxxxxx…');
  });

  it('is undefined for a missing or blank prompt, so no tooltip renders', () => {
    expect(imagePromptSummary(undefined)).toBeUndefined();
    expect(imagePromptSummary('   ')).toBeUndefined();
  });
});

describe('GeneratedImage tooltip', () => {
  function parts(prompt?: string) {
    const event: Extract<ResponseEvent, { type: 'image' }> = {
      type: 'image', base64: 'b64bytes', mime_type: 'image/jpeg', ...(prompt ? { prompt } : {}),
    };
    const wrapper = GeneratedImage({ event }) as VNode<{ 'data-tooltip'?: string; children?: unknown }>;
    const img = (wrapper.props as { children: VNode<{ 'data-tooltip'?: string; alt?: string }> }).children;
    return { wrapper, img };
  }

  /** Placement: the wrapper is a block as wide as the whole response column, so
   *  a tooltip anchored there centers over empty space beside the picture. It
   *  belongs on the `<img>`, whose rect is the picture itself. */
  it('anchors to the image, never to the full-width wrapper', () => {
    const { wrapper, img } = parts(PROMPT);
    expect(wrapper.props['data-tooltip']).toBeUndefined();
    expect(img.props['data-tooltip']).toBe(PROMPT);
  });

  it('describes the image with its prompt, in the tooltip and the alt text', () => {
    const { img } = parts(PROMPT);
    expect(img.props['data-tooltip']).toBe(PROMPT);
    expect(img.props.alt).toBe(PROMPT);
  });

  it('renders no tooltip at all when the prompt is unknown', () => {
    const { wrapper, img } = parts();
    expect(wrapper.props['data-tooltip']).toBeUndefined();
    expect(img.props['data-tooltip']).toBeUndefined();
    expect(img.props.alt).toBe('Generated image');
  });

  it('never renders a thread:N reference', () => {
    const { img } = parts(PROMPT);
    expect(img.props['data-tooltip']).not.toMatch(/thread:\d/);
  });
});
