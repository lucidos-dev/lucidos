import { signal } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { answerThreadQuestion, retryUnsentPick } from '../../store/actions/chat-claude-code';
import { recoverQuestionPick } from '../../store/actions/questionPickRecovery';
import { createTapGate } from '../../utils/tapGesture';
import { pendingAnswers, unsentPicks, type UnsentPick } from '../../store/pendingDecisions';
import { usePickMark, type PickMark } from '../../hooks/usePickMark';
import { renderMarkdown, renderMarkdownInline } from '../../utils/renderMarkdown';
import { CHOICE_CARD_ROLE, handAnsweredCardFocusToPrompt, handleChoiceCardKeyDown, seedChoiceCardFocus } from './choiceCardNav';
import { UserMessageBody } from './chat-exchange-parts';
import { followAnsweredQuestion, followConfirmedCard } from './scrollState';
import type { AnswerKind, QuestionOption, TypedAnswer } from '../../store/thread-events';
import { discardUnsentAnswers, retryUnsentMessage } from '../../store/actions/chat';
import { RetryIcon } from '../shared/icons';
import { Disclosure } from '../shared/Disclosure';
import { unsentMessages } from '../../store/unsentMessages';
import type { SendFailure } from '../../store/actions/sendRetry';
import { OptionWidgetFrame } from '../widgets/OptionWidgetFrame';
import { ReplyHtml, hasWidgetEmbedSlot } from '../widgets/useWidgetEmbedSlots';

export type ResolvedAnswer = AnswerKind;

export interface QuestionBodyProps {
  threadId: string;
  eventId?: string;
  toolUseId: string;
  question: string;
  options: QuestionOption[];
  multiSelect?: boolean;
  grantSelection?: boolean;
  resolved?: ResolvedAnswer;
  typedAnswer?: TypedAnswer;
  terminated?: boolean;
}

export const multiSelectedByToolUse = signal<Map<string, string[]>>(new Map());

export function getMultiSelectedIds(toolUseId: string): string[] {
  return multiSelectedByToolUse.value.get(toolUseId) ?? [];
}

export function setMultiSelectedIds(toolUseId: string, ids: string[], keepEmpty = false): void {
  const map = multiSelectedByToolUse.value;
  if (!keepEmpty && ids.length === 0 && !map.has(toolUseId)) return;
  const next = new Map(map);
  if (!keepEmpty && ids.length === 0) next.delete(toolUseId);
  else next.set(toolUseId, ids);
  multiSelectedByToolUse.value = next;
}

function clearMultiSelected(toolUseId: string): void {
  setMultiSelectedIds(toolUseId, []);
}

export function OptionIndicator({
  multiSelect,
  selected,
}: {
  multiSelect: boolean;
  selected: boolean;
}) {
  const shape = multiSelect ? 'question-option-indicator-checkbox' : 'question-option-indicator-radio';
  const sel = selected ? ' question-option-indicator-selected' : '';
  return <span class={`question-option-indicator ${shape}${sel}`} aria-hidden="true" />;
}

function OptionContent({
  option,
  multiSelect,
  selected,
  mark = 'mark',
}: {
  option: QuestionOption;
  multiSelect: boolean;
  selected: boolean;
  mark?: PickMark;
}) {
  return (
    <>
      {selected && mark === 'spinner'
        ? <span class="mini-spinner question-option-sending" aria-hidden="true" />
        : <OptionIndicator multiSelect={multiSelect} selected={selected && mark === 'mark'} />}
      <span class="question-option-label">{option.label}</span>
      {option.description && (
        <span
          class="question-option-desc authored-html"
          dangerouslySetInnerHTML={{ __html: renderMarkdownInline(option.description) }}
        />
      )}
      {option.preview && (
        <span
          class="question-option-preview authored-html"
          dangerouslySetInnerHTML={{ __html: renderMarkdownInline(option.preview) }}
        />
      )}
    </>
  );
}

function OptionSlot({ option, state, children }: {
  option: QuestionOption;
  state?: string;
  children: ComponentChildren;
}) {
  if (!option.widget) return <>{children}</>;
  return (
    <div class={state ? `question-option-boxed ${state}` : 'question-option-boxed'} data-option-id={option.id}>
      {children}
      <OptionWidgetFrame embed={option.widget} />
    </div>
  );
}

export function QuestionText({ question, threadId, eventId }: { question: string; threadId: string; eventId?: string }) {
  const html = renderMarkdown(question);
  if (hasWidgetEmbedSlot(html)) {
    return <ReplyHtml html={html} final threadId={threadId} eventId={eventId} class="question-text markdown-content" />;
  }
  return (
    <div
      class="question-text markdown-content authored-html"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

export function useAnswerLifecycle(toolUseId: string, resolved: ResolvedAnswer | undefined, terminated: boolean | undefined): UnsentPick | undefined {
  const unsentPick = unsentPicks.map.value.get(toolUseId);
  useEffect(() => {
    if ((!resolved && !terminated) || !unsentPick) return;
    if (unsentPick.answer.kind === 'FreeText' || (unsentPick.answer.kind === 'MultiSelected' && (unsentPick.answer.text || unsentPick.answer.image_hashes?.length))) {
      recoverQuestionPick(toolUseId, unsentPick);
    } else {
      unsentPicks.clear(toolUseId);
    }
  }, [resolved, terminated, unsentPick, toolUseId]);
  useEffect(() => {
    if (!resolved) return;
    pendingAnswers.clear(toolUseId);
    clearMultiSelected(toolUseId);
    followConfirmedCard(toolUseId);
  }, [resolved, toolUseId]);
  return unsentPick;
}

export async function pickAnswer(
  threadId: string,
  toolUseId: string,
  answer: AnswerKind,
  follow: (toolUseId: string) => void = followAnsweredQuestion,
): Promise<void> {
  if (pendingAnswers.map.peek().has(toolUseId)) return;
  handAnsweredCardFocusToPrompt();
  pendingAnswers.set(toolUseId, answer);
  follow(toolUseId);
  const outcome = await answerThreadQuestion(threadId, toolUseId, answer);
  if (outcome === 'sent') discardUnsentAnswers(threadId, toolUseId);
  else pendingAnswers.clear(toolUseId);
}

export function QuestionBody({ threadId, eventId, toolUseId, question, options, multiSelect, grantSelection, resolved, typedAnswer, terminated }: QuestionBodyProps) {
  const saveGate = useMemo(() => createTapGate(), []);
  const unsentPick = useAnswerLifecycle(toolUseId, resolved, terminated);

  const typedSending: ResolvedAnswer | undefined = typedAnswer?.state === 'sending'
    ? { kind: 'FreeText', text: typedAnswer.text, image_hashes: typedAnswer.image_hashes }
    : undefined;
  const pending = terminated ? undefined : pendingAnswers.map.value.get(toolUseId) ?? typedSending;
  const effective = resolved ?? pending;
  const pickMark = usePickMark(!!resolved, !!pending);
  if (effective) {
    const mark = multiSelect ? 'mark' : pickMark;
    return <AnsweredBody threadId={threadId} eventId={eventId} toolUseId={toolUseId} question={question} options={options} multiSelect={multiSelect} resolved={effective} mark={mark} />;
  }
  if (terminated) {
    return <TerminatedQuestionBody threadId={threadId} eventId={eventId} question={question} options={options} multiSelect={multiSelect} typedAnswer={typedAnswer} />;
  }
  const typedBlock = typedAnswer && typedAnswerBlock(typedAnswer);

  const onPick = (answer: AnswerKind) => void pickAnswer(threadId, toolUseId, answer);

  if (multiSelect) {
    const defaults = grantSelection ? options.map(option => option.id) : [];
    const selected = multiSelectedByToolUse.value.get(toolUseId) ?? defaults;
    const toggle = (id: string) => {
      const current = multiSelectedByToolUse.peek().get(toolUseId) ?? defaults;
      setMultiSelectedIds(
        toolUseId,
        current.includes(id) ? current.filter(chosen => chosen !== id) : [...current, id],
        grantSelection,
      );
    };
    return (
      <div class="question-body protected-surface" data-tool-use-id={toolUseId}>
        <QuestionText question={question} threadId={threadId} eventId={eventId} />
        {options.length > 0 && (
          <LiveOptions
            toolUseId={toolUseId}
            options={options}
            selectedIds={selected}
            unsent={unsentPick}
            onActivate={toggle}
          />
        )}
        {grantSelection && (
          <div class="permission-actions">
            {unsentPick?.answer.kind === 'MultiSelected' && unsentPick.answer.option_ids.length === 0 && (
              <UnsentNotice failure={unsentPick.failure} refusal={unsentPick.refusal} onRetry={() => void retryPick(threadId, toolUseId)} />
            )}
            <button
              type="button"
              class="action-btn action-btn-confirm"
              onPointerDown={event => saveGate.down(event)}
              onPointerMove={event => saveGate.move(event)}
              onPointerCancel={() => saveGate.cancel()}
              onClick={() => {
                if (!saveGate.isTap()) return;
                const option_ids = multiSelectedByToolUse.peek().get(toolUseId) ?? defaults;
                void onPick({ kind: 'MultiSelected', option_ids });
              }}
            >
              Save grants
            </button>
          </div>
        )}
        {unsentPick && unsentPickTextBlock(unsentPick, toolUseId)}
        {typedBlock}
      </div>
    );
  }

  return (
    <div class="question-body protected-surface" data-tool-use-id={toolUseId}>
      <QuestionText question={question} threadId={threadId} eventId={eventId} />
      {options.length > 0 && (
        <LiveOptions toolUseId={toolUseId} options={options} unsent={unsentPick} onActivate={option_id => void onPick({ kind: 'Selected', option_id })} />
      )}
      {typedBlock}
    </div>
  );
}

export function typedAnswerBlock(answer: TypedAnswer) {
  const unsentEventId = answer.state === 'unsent' ? answer.unsentEventId : undefined;
  return (
    <TypedAnswerBlock
      text={answer.text}
      imageHashes={answer.image_hashes}
      unsent={unsentEventId === undefined ? undefined : {
        failure: unsentMessages.value.get(unsentEventId)?.failure,
        refusal: unsentMessages.value.get(unsentEventId)?.refusal,
        onRetry: () => void retryUnsentMessage(unsentEventId),
      }}
    />
  );
}

export function TypedAnswerBlock({
  text,
  imageHashes,
  sending = false,
  unsent,
}: {
  text: string | undefined;
  imageHashes: string[];
  sending?: boolean;
  unsent?: UnsentNoticeProps;
}) {
  return (
    <div class="question-freetext">
      <span class="question-freetext-label">
        {sending && <span class="mini-spinner question-freetext-sending" aria-hidden="true" />}
        Your answer
      </span>
      <div class="user-bubble question-freetext-text">
        <UserMessageBody html={text ? renderMarkdown(text) : ''} imageHashes={imageHashes} />
      </div>
      {unsent && <UnsentNotice {...unsent} />}
    </div>
  );
}

export async function retryPick(threadId: string, toolUseId: string): Promise<void> {
  if (await retryUnsentPick(toolUseId) === 'sent') discardUnsentAnswers(threadId, toolUseId);
}

function pickedOptionIds(answer: AnswerKind): string[] {
  if (answer.kind === 'Selected') return [answer.option_id];
  if (answer.kind === 'MultiSelected') return answer.option_ids;
  return [];
}

function unsentPickTextBlock(pick: UnsentPick, toolUseId: string) {
  if (pick.answer.kind !== 'MultiSelected') return null;
  const { text, image_hashes: imageHashes = [], option_ids: optionIds } = pick.answer;
  if (!text && imageHashes.length === 0) return null;
  return (
    <TypedAnswerBlock
      text={text}
      imageHashes={imageHashes}
      unsent={optionIds.length > 0 ? undefined : { failure: pick.failure, refusal: pick.refusal, onRetry: () => void retryPick(pick.threadId, toolUseId) }}
    />
  );
}

export interface UnsentNoticeProps {
  failure: SendFailure | undefined;
  refusal?: UnsentPick['refusal'];
  onRetry: () => void;
}

function sendFailureDetail(failure: SendFailure): string {
  const tries = failure.attempts === 1 ? 'try' : 'tries';
  return `Lucidos did not answer after ${failure.attempts} ${tries}. Last error: ${failure.reason}.`;
}

export function UnsentNotice({ failure, refusal, onRetry }: UnsentNoticeProps) {
  const [open, setOpen] = useState(false);
  return (
    <div class="question-unsent">
      <div class="question-unsent-row">
        <button
          type="button"
          class="question-unsent-toggle"
          aria-expanded={open}
          data-tooltip={open ? 'Hide details' : 'Show details'}
          onClick={() => setOpen(!open)}
        >
          Not sent
        </button>
        <button
          type="button"
          class="icon-btn question-unsent-retry"
          aria-label="Retry sending your answer"
          data-tooltip="Retry"
          onClick={onRetry}
        >
          <RetryIcon />
        </button>
      </div>
      <Disclosure open={open} bodyClass="question-unsent-detail">
        {refusal?.message ?? (failure ? sendFailureDetail(failure) : 'Lucidos did not answer, so this answer was not sent.')}
      </Disclosure>
    </div>
  );
}

function LiveOptions({
  toolUseId,
  options,
  selectedIds,
  unsent,
  onActivate,
}: {
  toolUseId: string;
  options: QuestionBodyProps['options'];
  selectedIds?: string[];
  unsent?: UnsentPick;
  onActivate: (id: string) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const unsentIds = unsent ? pickedOptionIds(unsent.answer) : [];
  const noticeOn = options.find(opt => unsentIds.includes(opt.id))?.id;
  useEffect(() => { seedChoiceCardFocus(ref.current, toolUseId); }, [toolUseId]);
  return (
    <div
      class="question-options"
      data-role={CHOICE_CARD_ROLE}
      ref={ref}
      onKeyDown={(e) => handleChoiceCardKeyDown(e, ref.current)}
    >
      {options.map(opt => {
        const pressed = selectedIds ? selectedIds.includes(opt.id) : undefined;
        const unsentHere = unsentIds.includes(opt.id);
        const button = (
          <OptionButton
            option={opt}
            pressed={pressed}
            unsent={unsentHere}
            onActivate={onActivate}
          />
        );
        return (
          <OptionSlot key={opt.id} option={opt} state={pressed || unsentHere ? 'question-option-selected' : undefined}>
            {unsent && opt.id === noticeOn ? (
              <div class="question-option-unsent">
                {button}
                <UnsentNotice failure={unsent.failure} refusal={unsent.refusal} onRetry={() => void retryPick(unsent.threadId, toolUseId)} />
              </div>
            ) : button}
          </OptionSlot>
        );
      })}
    </div>
  );
}

function OptionButton({
  option,
  pressed,
  unsent = false,
  onActivate,
}: {
  option: QuestionOption;
  pressed?: boolean;
  unsent?: boolean;
  onActivate: (id: string) => void;
}) {
  const gate = useMemo(() => createTapGate(), []);
  const isToggle = pressed !== undefined;
  return (
    <button
      type="button"
      class={unsent ? 'question-option question-option-selected' : 'question-option'}
      aria-pressed={isToggle ? pressed : undefined}
      onPointerDown={e => gate.down(e)}
      onPointerMove={e => gate.move(e)}
      onPointerCancel={() => gate.cancel()}
      onClick={(e) => {
        if (!gate.isTap()) return;
        if (e.target instanceof Element && e.target.closest('.image-scroll-wrapper')) return;
        onActivate(option.id);
      }}
      aria-label={`${isToggle ? 'Toggle' : 'Answer'}: ${option.label}`}
    >
      <OptionContent option={option} multiSelect={isToggle} selected={!!pressed || unsent} />
    </button>
  );
}

export function typedAnswerText(answer: AnswerKind): string | undefined {
  return answer.kind === 'FreeText' || answer.kind === 'MultiSelected' ? answer.text : undefined;
}

export const QUESTION_SUPERSEDED_NOTE = 'Replaced by your next message';

export function AnsweredBody({
  threadId,
  eventId,
  toolUseId,
  question,
  options,
  multiSelect,
  resolved,
  mark = 'mark',
}: {
  threadId: string;
  eventId?: string;
  toolUseId: string;
  question: string;
  options: QuestionBodyProps['options'];
  multiSelect: boolean | undefined;
  resolved: ResolvedAnswer;
  mark?: PickMark;
}) {
  const isSelected = (id: string) =>
    (resolved.kind === 'Selected' && resolved.option_id === id) ||
    (resolved.kind === 'MultiSelected' && resolved.option_ids.includes(id));
  const customText = typedAnswerText(resolved);
  const imageHashes =
    resolved.kind === 'FreeText' || resolved.kind === 'MultiSelected' ? resolved.image_hashes ?? [] : [];
  return (
    <div class="question-body question-body-answered protected-surface" data-tool-use-id={toolUseId}>
      <QuestionText question={question} threadId={threadId} eventId={eventId} />
      {options.length > 0 && (
        <div class="question-options">
          {options.map(opt => {
            const state = isSelected(opt.id) ? 'question-option-selected' : 'question-option-dimmed';
            return (
              <OptionSlot key={opt.id} option={opt} state={state}>
                <div class={opt.widget ? 'question-option-static' : `question-option-static ${state}`}>
                  <OptionContent option={opt} multiSelect={!!multiSelect} selected={isSelected(opt.id)} mark={mark} />
                </div>
              </OptionSlot>
            );
          })}
        </div>
      )}
      {((customText && customText.length > 0) || imageHashes.length > 0) && (
        <TypedAnswerBlock text={customText} imageHashes={imageHashes} sending={mark === 'spinner' && resolved.kind === 'FreeText'} />
      )}
      {resolved.kind === 'Canceled' && (
        <button
          type="button"
          class="action-btn action-btn-danger permission-btn-picked question-cancel-picked"
          disabled
        >
          <span class="permission-btn-check" aria-hidden="true">✓ </span>
          Cancel
        </button>
      )}
      {resolved.kind === 'Superseded' && (
        <span class="question-superseded-note">{QUESTION_SUPERSEDED_NOTE}</span>
      )}
    </div>
  );
}

export function TerminatedQuestionBody({
  threadId,
  eventId,
  question,
  options,
  multiSelect,
  typedAnswer,
}: {
  threadId: string;
  eventId?: string;
  question: string;
  options: QuestionBodyProps['options'];
  multiSelect: boolean | undefined;
  typedAnswer?: TypedAnswer;
}) {
  return (
    <div class="question-body question-body-terminated protected-surface">
      <QuestionText question={question} threadId={threadId} eventId={eventId} />
      {options.length > 0 && (
        <div class="question-options">
          {options.map(opt => (
            <OptionSlot key={opt.id} option={opt}>
              <button type="button" class="question-option" disabled>
                <OptionContent option={opt} multiSelect={!!multiSelect} selected={false} />
              </button>
            </OptionSlot>
          ))}
        </div>
      )}
      {typedAnswer && typedAnswerBlock(typedAnswer)}
    </div>
  );
}
