// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import * as QuestionCardModule from '../QuestionCard';
import * as questionActions from '../../../store/actions/chat-claude-code';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import {
  OptionIndicator,
  AnsweredBody,
  QuestionBody,
  TerminatedQuestionBody,
  getMultiSelectedIds,
  setMultiSelectedIds,
} from '../QuestionCard';
import { PLACEHOLDER_ANSWERING } from '../prompt-input-helpers';
import { ANSWER_CANCEL_TOOLTIP } from '../PromptInput';
import { vnodeToText } from './vnodeToText';

describe('OptionIndicator', () => {
  it('renders a radio-style indicator for single-select (multiSelect=false)', () => {
    const text = vnodeToText(OptionIndicator({ multiSelect: false, selected: false }));
    expect(text).toContain('question-option-indicator');
    expect(text).toContain('question-option-indicator-radio');
    expect(text).not.toContain('question-option-indicator-checkbox');
  });

  it('renders a checkbox-style indicator for multi-select', () => {
    const text = vnodeToText(OptionIndicator({ multiSelect: true, selected: false }));
    expect(text).toContain('question-option-indicator-checkbox');
    expect(text).not.toContain('question-option-indicator-radio');
  });

  it('marks the indicator as selected when selected=true', () => {
    const single = vnodeToText(OptionIndicator({ multiSelect: false, selected: true }));
    const multi = vnodeToText(OptionIndicator({ multiSelect: true, selected: true }));
    expect(single).toContain('question-option-indicator-selected');
    expect(multi).toContain('question-option-indicator-selected');
  });
});

describe('the card carries no hint of its own', () => {
  const SOURCE = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../QuestionCard.tsx'),
    'utf8',
  );

  it('renders no hint element and exports no hint symbol', () => {
    expect(SOURCE).not.toContain('class="question-hint"');
    expect(Object.keys(QuestionCardModule).filter(k => /hint/i.test(k))).toEqual([]);
  });

  it('leaves the typing escape to the prompt placeholder, in one short line', () => {
    expect(PLACEHOLDER_ANSWERING).toContain('Type');
    expect(PLACEHOLDER_ANSWERING).toContain('custom answer');
    expect(PLACEHOLDER_ANSWERING.length).toBeLessThan(32);
    expect(PLACEHOLDER_ANSWERING).not.toContain('.');
  });

  it('leaves the Cancel escape to the prompt row Cancel tooltip', () => {
    expect(ANSWER_CANCEL_TOOLTIP).toContain('Cancel');
    expect(ANSWER_CANCEL_TOOLTIP).toContain('ask something else');
  });

  it('wires that tooltip onto the lone Cancel in PromptInput', () => {
    const promptSource = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), '../PromptInput.tsx'),
      'utf8',
    );
    expect(promptSource).toMatch(
      /data-tooltip=\{answeringQuestionCard\s*\?\s*ANSWER_CANCEL_TOOLTIP\s*:\s*tooltipWithShortcut\('Stop', 'stopThread'\)\}/,
    );
  });
});

describe('AnsweredBody — Canceled state', () => {
  it('renders a disabled red Cancel button instead of the orange canceled badge', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: 'q',
      options: [{ id: 'a', label: 'A' }],
      multiSelect: false,
      resolved: { kind: 'Canceled' },
    }));
    expect(text).toContain('action-btn');
    expect(text).toContain('action-btn-danger');
    expect(text).toContain('question-cancel-picked');
    expect(text).toContain(' disabled');
    expect(text).toContain('Cancel');
    expect(text).toContain('✓');
    expect(text).not.toContain('question-canceled-badge');
  });

  it('renders a plain note for a Superseded answer, never the Cancel affordance', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: 'q',
      options: [{ id: 'a', label: 'A' }],
      multiSelect: false,
      resolved: { kind: 'Superseded' },
    }));
    expect(text).toContain('question-superseded-note');
    expect(text).toContain('Replaced by your next message');
    expect(text).not.toContain('question-cancel-picked');
    expect(text).not.toContain('Cancel');
  });

  it('does not render the cancel-as-picked button for a Selected answer', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: 'q',
      options: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }],
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'a' },
    }));
    expect(text).not.toContain('question-cancel-picked');
    expect(text).not.toContain('action-btn-danger');
  });

  it('does not render the cancel-as-picked button for a FreeText answer', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: 'q',
      options: [],
      multiSelect: false,
      resolved: { kind: 'FreeText', text: 'hello' },
    }));
    expect(text).not.toContain('question-cancel-picked');
    expect(text).not.toContain('action-btn-danger');
    expect(text).toContain('hello');
  });
});

describe('AnsweredBody: the custom answer wears the user bubble', () => {
  const LABEL_THEN_BUBBLE =
    /class="question-freetext-label">Your answer<\/span><div class="[^"]*\buser-bubble\b[^"]*">/;

  it('renders a FreeText answer in a user bubble under its header', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: 'q',
      options: [],
      multiSelect: false,
      resolved: { kind: 'FreeText', text: 'neither, ask me later' },
    }));
    expect(text).toMatch(LABEL_THEN_BUBBLE);
    expect(text).toContain('neither, ask me later');
  });

  it('renders the freetext typed alongside a MultiSelected pick the same way', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: 'q',
      options: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }],
      multiSelect: true,
      resolved: { kind: 'MultiSelected', option_ids: ['a'], text: 'and also this' },
    }));
    expect(text).toMatch(LABEL_THEN_BUBBLE);
    expect(text).toContain('and also this');
  });

  it('renders the typed answer as markdown, so a fence becomes a code block', () => {
    const host = document.createElement('div');
    render(
      <AnsweredBody
        threadId="t"
        toolUseId="tool-1"
        question="q"
        options={[]}
        multiSelect={false}
        resolved={{ kind: 'FreeText', text: 'Here:\n\n```\n{"a": 1}\n```' }}
      />,
      host,
    );
    const bubble = host.querySelector('.user-bubble');
    expect(bubble?.querySelector('pre code')?.textContent).toBe('{"a": 1}');
    expect(bubble?.textContent).not.toContain('```');
    render(null, host);
  });

  it('renders no freetext block at all when the answer carries no text', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: 'q',
      options: [{ id: 'a', label: 'A' }],
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'a' },
    }));
    expect(text).not.toContain('question-freetext');
    expect(text).not.toContain('Your answer');
  });
});

describe('a dead card says so to the scroll landing', () => {
  it('an answered body wears question-body-answered', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: 'q',
      options: [],
      multiSelect: false,
      resolved: { kind: 'FreeText', text: 'hello' },
    }));
    expect(text).toContain('question-body-answered');
  });

  it('a terminated body wears question-body-terminated', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      threadId: 't',
      question: 'q',
      options: [{ id: 'a', label: 'A' }],
      multiSelect: false,
    }));
    expect(text).toContain('question-body-terminated');
  });
});

describe('TerminatedQuestionBody', () => {
  it('renders every option as a disabled button (single-select)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      threadId: 't',
      question: 'Pick a flavor',
      options: [
        { id: 'a', label: 'Vanilla' },
        { id: 'b', label: 'Chocolate' },
      ],
      multiSelect: false,
    }));
    expect(text).toContain('Pick a flavor');
    expect(text).toContain('Vanilla');
    expect(text).toContain('Chocolate');
    const disabledCount = (text.match(/ disabled/g) ?? []).length;
    expect(disabledCount).toBe(2);
    expect(text).not.toContain('question-cancel-picked');
    expect(text).not.toContain('question-option-selected');
  });

  it('renders every option as a disabled button (multi-select)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      threadId: 't',
      question: 'Pick any',
      options: [
        { id: 'a', label: 'A' },
        { id: 'b', label: 'B' },
        { id: 'c', label: 'C' },
      ],
      multiSelect: true,
    }));
    const disabledCount = (text.match(/ disabled/g) ?? []).length;
    expect(disabledCount).toBe(3);
  });

  it('renders only the question text when the question had no options (freetext-only ask)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      threadId: 't',
      question: 'What do you think?',
      options: [],
      multiSelect: false,
    }));
    expect(text).toContain('What do you think?');
    expect(text).not.toContain(' disabled');
  });
});

describe('question text: URL linkification', () => {
  const QUESTION =
    'Draft PR #1488 is ready: https://github.com/example-org/example-repo/pull/1488. Mark it ready?';

  it('renders a bare URL in the question as a link (terminated body)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      threadId: 't',
      question: QUESTION,
      options: [{ id: 'a', label: 'Yes' }],
      multiSelect: false,
    }));
    expect(text).toContain('<a href="https://github.com/example-org/example-repo/pull/1488">');
  });

  it('renders the URL as a link in the answered body too', () => {
    const text = vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tool-1',
      question: QUESTION,
      options: [{ id: 'a', label: 'Yes' }],
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'a' },
    }));
    expect(text).toContain('href="https://github.com/example-org/example-repo/pull/1488"');
  });
});

describe('question text: block markdown', () => {
  const QUESTION = [
    'Findings on option A: it is doable.',
    '',
    '- **Security:** the new endpoint needs a shared secret.',
    '- **Retries:** use the landing event id as the repeat key.',
    '',
    'What should happen next?',
  ].join('\n');

  it('renders paragraphs and lists as blocks, in a markdown surface', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      threadId: 't',
      question: QUESTION,
      options: [{ id: 'a', label: 'Update the plan' }],
      multiSelect: false,
    }));
    expect(text).toContain('question-text markdown-content');
    expect(text).toContain('<p>Findings on option A: it is doable.</p>');
    expect(text).toMatch(/<ul>\s*<li><strong>Security:<\/strong>/);
    expect(text).toContain('<p>What should happen next?</p>');
  });
});

describe('an option preview', () => {
  const options = [
    { id: 'opt-0', label: 'D', preview: '![D](artifacts/variant-d.png)' },
    { id: 'opt-1', label: 'C', preview: '  Sent  21:30\n  ^^^^  dim' },
    { id: 'opt-2', label: 'B' },
  ];

  const expectPictureAndSample = (html: string) => {
    expect(html).toContain('question-option-preview');
    expect(html).toMatch(/<img[^>]*src="[^"]*\/data\/artifacts\/variant-d\.png"/);
    expect(html).not.toContain('src="artifacts/');
    expect(html).toContain('  Sent  21:30<br>  ^^^^  dim');
    expect(html.match(/question-option-preview/g)).toHaveLength(2);
  };

  it('shows on the live card, with no link inside the option button', () => {
    const host = document.createElement('div');
    render(
      <QuestionBody
        threadId="t"
        toolUseId="tu-preview"
        question="Which one should I build?"
        options={[...options, { id: 'opt-3', label: 'L', preview: '[x](https://example.com)' }]}
      />,
      host,
    );
    const buttons = Array.from(host.querySelectorAll('button.question-option'));
    expect(buttons).toHaveLength(4);
    expectPictureAndSample(buttons.slice(0, 3).map(b => b.outerHTML).join(''));
    expect(host.querySelector('button.question-option a')).toBeNull();
    render(null, host);
  });

  it('lets a tap on its picture open the viewer instead of choosing', () => {
    const host = document.createElement('div');
    document.body.append(host);
    render(
      <QuestionBody
        threadId="t"
        toolUseId="tu-preview-tap"
        question="Which one?"
        options={options}
        multiSelect
      />,
      host,
    );
    const button = host.querySelector<HTMLButtonElement>('button.question-option')!;
    const img = button.querySelector<HTMLImageElement>('.image-scroll-wrapper > img');
    expect(img, 'the picture sits in the same wrapper as a reply image').toBeTruthy();

    img!.click();
    expect(getMultiSelectedIds('tu-preview-tap')).toEqual([]);

    img!.closest<HTMLElement>('.image-scroll-wrapper')!.click();
    expect(getMultiSelectedIds('tu-preview-tap')).toEqual([]);

    button.querySelector<HTMLElement>('.question-option-label')!.click();
    expect(getMultiSelectedIds('tu-preview-tap')).toEqual(['opt-0']);

    setMultiSelectedIds('tu-preview-tap', []);
    render(null, host);
    host.remove();
  });

  it('does not sink the option while its picture is pressed', () => {
    const css = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), '../../../styles/chat/response.css'),
      'utf8',
    );
    expect(css).toContain('.question-option:active:not(:has(.image-scroll-wrapper:active)) {');
    expect(css).not.toMatch(/\.question-option:active\s*\{/);
  });

  it('shows on the answered card', () => {
    expectPictureAndSample(vnodeToText(AnsweredBody({
      threadId: 't',
      toolUseId: 'tu-preview',
      question: 'q',
      options,
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'opt-0' },
    })));
  });

  it('shows on the terminated card', () => {
    expectPictureAndSample(vnodeToText(TerminatedQuestionBody({
      threadId: 't',
      question: 'q',
      options,
      multiSelect: false,
    })));
  });
});

describe('an option with a widget', () => {
  const player = {
    id: 'lucidos-sound-player', name: 'Sound player', description: '', reveal: 'on-load' as const,
    kind: 'widget' as const, reusable: true, built_in: true, sizes: ['medium' as const], params: { clip: { description: 'What to play', required: true } },
  };
  const options = [
    { id: 'opt-0', label: 'Marin', description: 'Warm, unhurried', widget: { app_id: player.id, params: { clip: 'marin.mp3' }, label: 'Marin' } },
    { id: 'opt-1', label: 'Ash' },
  ];

  async function withPlayerKnown(run: (host: HTMLElement) => void): Promise<void> {
    const { appsById: widgetApps } = await import('../../../store/appsById');
    widgetApps.value = new Map([[player.id, { status: 'loaded', data: player }]]);
    const host = document.createElement('div');
    document.body.append(host);
    try {
      run(host);
    } finally {
      render(null, host);
      host.remove();
      widgetApps.value = new Map();
    }
  }

  it('draws the frame beside the button, and only buttons are choices', async () => {
    await withPlayerKnown((host) => {
      render(<QuestionBody threadId="t" toolUseId="tu-widget" question="Which voice?" options={options} multiSelect />, host);
      const box = host.querySelector('.question-option-boxed')!;
      const button = box.querySelector(':scope > button.question-option')!;
      const frame = box.querySelector(':scope > .widget-embed')!;
      expect(button && frame).toBeTruthy();
      expect(button.contains(frame), 'the frame is never inside the button').toBe(false);
      expect(button.compareDocumentPosition(frame) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(host.querySelectorAll('[data-role="card-choices"] button')).toHaveLength(2);
      expect(button.querySelector('.question-option-desc')!.textContent).toBe('Warm, unhurried');

      (frame as HTMLElement).click();
      expect(getMultiSelectedIds('tu-widget'), 'a tap on the widget picks nothing').toEqual([]);
      button.querySelector<HTMLElement>('.question-option-label')!.click();
      expect(getMultiSelectedIds('tu-widget')).toEqual(['opt-0']);
      setMultiSelectedIds('tu-widget', []);
    });
  });

  it('keeps every widget on an answered card and a terminated one', async () => {
    await withPlayerKnown((host) => {
      render(
        <AnsweredBody threadId="t" toolUseId="tu-a" question="Which voice?" options={options} multiSelect={false}
          resolved={{ kind: 'Selected', option_id: 'opt-1' }} />,
        host,
      );
      const box = host.querySelector('.question-option-boxed')!;
      expect(box.querySelector('.widget-embed')).toBeTruthy();
      expect(box.classList.contains('question-option-dimmed'), 'the box dims, not the row twice').toBe(true);
      expect(box.querySelector('.question-option-static.question-option-dimmed')).toBeNull();

      render(<TerminatedQuestionBody threadId="t" question="Which voice?" options={options} multiSelect={false} />, host);
      expect(host.querySelector('.question-option-boxed .widget-embed')).toBeTruthy();
    });
  });
});

describe('a widget embed in the question text', () => {
  const tapper = {
    id: 'tap-counter', name: 'Tap counter', description: '', reveal: 'on-load' as const,
    kind: 'widget' as const, reusable: false, origin_thread_id: 't', built_in: false, sizes: ['medium' as const],
  };
  const question = 'Here is the widget:\n\n![Tap counter](app:tap-counter)\n\nKeep it?';
  const options = [{ id: 'opt-0', label: 'Keep' }, { id: 'opt-1', label: 'Drop' }];

  async function withTapperKnown(run: (host: HTMLElement) => Promise<void>): Promise<void> {
    const { appsById } = await import('../../../store/appsById');
    appsById.value = new Map([[tapper.id, { status: 'loaded', data: tapper }]]);
    const host = document.createElement('div');
    document.body.append(host);
    try {
      await run(host);
    } finally {
      render(null, host);
      host.remove();
      appsById.value = new Map();
    }
  }

  it('draws on the live, answered and terminated cards', async () => {
    await withTapperKnown(async (host) => {
      await act(() => render(<QuestionBody threadId="t" toolUseId="tu-text" question={question} options={options} />, host));
      expect(host.querySelector('.question-text .inline-widget:not(.inline-widget-gone)')).toBeTruthy();

      await act(() => render(
        <AnsweredBody threadId="t" toolUseId="tu-text" question={question} options={options} multiSelect={false}
          resolved={{ kind: 'Selected', option_id: 'opt-0' }} />,
        host,
      ));
      expect(host.querySelector('.question-text .inline-widget:not(.inline-widget-gone)')).toBeTruthy();

      await act(() => render(<TerminatedQuestionBody threadId="t" question={question} options={options} multiSelect={false} />, host));
      expect(host.querySelector('.question-text .inline-widget:not(.inline-widget-gone)')).toBeTruthy();
    });
  });

  it('keeps another thread\'s widget unavailable', async () => {
    await withTapperKnown(async (host) => {
      await act(() => render(<QuestionBody threadId="other" toolUseId="tu-other" question={question} options={options} />, host));
      expect(host.querySelector('.question-text .inline-widget-gone')).toBeTruthy();
    });
  });

  it('stays hook-free for text that only names the slot attribute', () => {
    for (const named of ['Mentions `data-widget-embed-slot` in code.', '<span data-widget-embed-slot="guess">x</span>']) {
      expect(vnodeToText(TerminatedQuestionBody({ threadId: 't', question: named, options, multiSelect: false }))).toContain('Keep');
    }
  });
});

it.each([
  { unchecked: [0], expected: ['git'] },
  { unchecked: [0, 1], expected: [] },
])('saves only checked review grants after remount ($expected)', async ({ unchecked, expected }) => {
  const root = document.createElement('div');
  const props = {
    threadId: 'trigger-review',
    toolUseId: 'trigger-review-options',
    question: 'Choose grants',
    options: [{ id: 'curl', label: 'curl' }, { id: 'git', label: 'git' }],
    multiSelect: true,
    grantSelection: true,
  };
  const submit = vi.spyOn(questionActions, 'answerThreadQuestion').mockResolvedValue('sent');
  try {
    const mount = () => act(() => render(<QuestionBody {...props} />, root));
    const toggles = () => [...root.querySelectorAll<HTMLButtonElement>('button[aria-pressed]')];
    mount();
    expect(toggles().map(node => node.getAttribute('aria-pressed'))).toEqual(['true', 'true']);
    for (const index of unchecked) act(() => toggles()[index]!.click());
    act(() => render(null, root));
    mount();
    expect(toggles().map(node => node.getAttribute('aria-pressed'))).toEqual(
      props.options.map(option => String(expected.includes(option.id))),
    );
    const button = [...root.querySelectorAll('button')].find(node => node.textContent === 'Save grants');
    await act(async () => button!.click());
    const answer = { kind: 'MultiSelected', option_ids: expected } as const;
    expect(submit).toHaveBeenCalledExactlyOnceWith(props.threadId, props.toolUseId, answer);
    act(() => render(<QuestionBody {...props} resolved={answer} />, root));
    expect([...root.querySelectorAll('.question-option-static.question-option-selected')]
      .map(node => node.textContent)).toEqual(expected);
  } finally {
    act(() => render(null, root));
    submit.mockRestore();
  }
});
