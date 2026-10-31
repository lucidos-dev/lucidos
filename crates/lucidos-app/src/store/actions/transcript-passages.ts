/**
 * What thread find counts: every text the transcript draws, read from the
 * thread's data with the code that draws it. Each passage names the element it
 * draws into and what hides that element, so a step can unfold it.
 *
 * Plan: `docs/plans/2026-10-25-find-all-thread-text.md`.
 */

import { render, type ComponentChildren } from 'preact';
import { collectPageText } from '@lucidos/find';
import { computeExchanges } from '../thread-events/exchange-grouping';
import {
  drawsInitiatorPanel, exchangeEngineLimitDetail, exchangeKey, exchangeUserMessage, findCommandPermissionResolution, findMcpPermissionResolution,
  findPermissionResolution, findQuestionAnswer, isUserBubbleEvent, type Exchange,
} from '../thread-events/exchange';
import { exchangeError } from '../thread-events/exchange-anchors';
import { exchangeResponseEvents } from '../thread-events/exchange-render';
import type { ThreadState } from '../thread-events/thread-meta';
import type { ResponseEvent } from '../types';
import type { SideQuestion } from '../sideQuestions';
import { getCollapsedVisibleEvents, headClampApplies, isMeaningfulText } from '../event-rendering';
import { renderMarkdown, renderMarkdownInline } from '../../utils/renderMarkdown';
import { QUESTION_SUPERSEDED_NOTE, typedAnswerText } from '../../components/chat/QuestionCard';
import {
  engineResolutionNote, renderCommandQuestion, renderMcpQuestion, renderQuestion,
} from '../../components/chat/PermissionCard';
import { stepRowText, waitDetail } from '../../components/chat/chat-exchange-parts';
import { agentExitReason } from '../../components/chat/agentExitCopy';

/** What a step to a passage unfolds before the passage is drawn. */
export interface Unfold {
  /** In the turn's initiator panel, which the reader can fold. */
  initiator?: true;
  /** Drawn only with the full response on. */
  details?: true;
  /** A step row, drawn only with steps on. */
  steps?: true;
  /** Inside this sub-agent group, which starts folded. */
  subAgent?: string;
  /** In this side question card, which the reader can fold. */
  sideQuestion?: string;
}

/** One searchable stretch of the transcript, as the plain text it draws. */
export interface TranscriptPassage {
  /** The turn's exchange key, which the render request names. */
  key: string;
  userSeq: number;
  /** Where it draws: every live match of the selector in its turn, or only
   *  the `nth`. `inFeed` searches the whole feed, for a side question drawn
   *  between turns. */
  where: { selector: string; nth?: number; inFeed?: true };
  unfold: Unfold;
  text: string;
}

/** How a passage's text is read. A run caches it, since the thread is
 *  searched on every keystroke and its settled passages do not change. */
export interface PassageText {
  markdown(md: string): string;
  inlineMarkdown(md: string): string;
  /** `key` names the drawing, for the cache. */
  drawing(key: string, draw: () => ComponentChildren): string;
}

let parseDoc: Document | null = null;

function parseRoot(): HTMLElement {
  parseDoc ??= document.implementation.createHTMLDocument('');
  return parseDoc.body;
}

/** Text read from markup in a document never shown. Blocks are told apart as
 *  the drawn transcript tells them. */
function htmlText(html: string): string {
  const root = parseRoot();
  root.innerHTML = html;
  return collectPageText(root, 'markup').text;
}

/** A drawing's text. Only plain markup comes here, never a component with
 *  effects: rendering it must change nothing outside the hidden document. */
function drawingText(draw: ComponentChildren): string {
  const root = parseRoot();
  root.replaceChildren();
  render(draw, root);
  const { text } = collectPageText(root, 'markup');
  render(null, root);
  return text;
}

export const markdownText = (md: string): string => htmlText(renderMarkdown(md));

/** Reads every text afresh. */
export const freshText: PassageText = {
  markdown: markdownText,
  inlineMarkdown: (md) => htmlText(renderMarkdownInline(md)),
  drawing: (_key, draw) => drawingText(draw()),
};

/** Reads each text once into `cache`. */
export function cachedText(cache: Map<string, string>): PassageText {
  const once = (key: string, read: () => string) => {
    let text = cache.get(key);
    if (text === undefined) cache.set(key, (text = read()));
    return text;
  };
  return {
    markdown: (md) => once(`md:${md}`, () => freshText.markdown(md)),
    inlineMarkdown: (md) => once(`inline:${md}`, () => freshText.inlineMarkdown(md)),
    drawing: (key, draw) => once(`drawing:${key}`, () => freshText.drawing(key, draw)),
  };
}

/** A value as a quoted CSS attribute string. */
const css = (value: string | number) => `"${String(value).replace(/["\\]/g, '\\$&')}"`;

/** A user's message, drawn as a bubble or as a plain panel (an API caller's
 *  or an agent's message). Never a side question or a card's typed answer. */
const USER_MESSAGE = '.initiator-panel .initiator-body .markdown-content';

/** Every passage of the thread, in reading order. Pure over the thread's
 *  data, so it counts history the transcript has not drawn. */
export function transcriptPassages(
  thread: ThreadState,
  read: PassageText,
  sideQuestions: readonly SideQuestion[] = [],
): TranscriptPassage[] {
  const exchanges = computeExchanges(thread);
  const owned = sideQuestionsByTurn(exchanges, sideQuestions);
  const passages: TranscriptPassage[] = [];
  for (const [i, exchange] of exchanges.entries()) {
    const at = { key: exchangeKey(exchange), userSeq: exchange.userSeq };
    const add = (where: TranscriptPassage['where'], unfold: Unfold, text: string) => {
      if (text.trim()) passages.push({ ...at, where, unfold, text });
    };
    if (drawsInitiatorPanel(exchange)) initiatorPassages(exchange, read, add);
    responsePassages(exchange, read, add);
    for (const card of owned.get(i) ?? []) sideQuestionPassages(card, read, add);
  }
  return passages;
}

type Add = (where: TranscriptPassage['where'], unfold: Unfold, text: string) => void;

const INITIATOR: Unfold = { initiator: true };

function initiatorPassages(exchange: Exchange, read: PassageText, add: Add): void {
  const ev = exchange.userEvent;
  if (isUserBubbleEvent(ev)) {
    add({ selector: USER_MESSAGE }, INITIATOR, read.markdown(exchangeUserMessage(exchange)));
    return;
  }
  switch (ev.type) {
    case 'UserQuestionAsked': {
      const card = '.initiator-body .question-body';
      add({ selector: `${card} .question-text` }, INITIATOR, read.markdown(ev.question));
      const options = ev.options ?? [];
      for (const [nth, option] of options.entries()) {
        add({ selector: `${card} .question-option-label`, nth }, INITIATOR, option.label);
      }
      for (const [nth, option] of options.filter((o) => o.description).entries()) {
        add({ selector: `${card} .question-option-desc`, nth }, INITIATOR, read.inlineMarkdown(option.description!));
      }
      const answer = findQuestionAnswer(exchange, ev.tool_use_id)?.answer;
      const typed = answer ? typedAnswerText(answer) : exchange.typedAnswer?.text;
      if (typed) add({ selector: `${card} .question-freetext-text` }, INITIATOR, read.markdown(typed));
      if (answer?.kind === 'Superseded') {
        add({ selector: `${card} .question-superseded-note` }, INITIATOR, QUESTION_SUPERSEDED_NOTE);
      }
      return;
    }
    case 'CodingAgentPermissionRequest':
      permissionPassages(ev.request_id, findPermissionResolution(exchange, ev.request_id), read,
        () => renderQuestion(ev.tool_name, ev.summary, ev.input), add);
      return;
    case 'CommandPermissionRequested':
      permissionPassages(ev.request_id, findCommandPermissionResolution(exchange, ev.request_id), read,
        () => renderCommandQuestion(ev.command, ev.summary), add);
      return;
    case 'McpPermissionRequested':
      permissionPassages(ev.request_id, findMcpPermissionResolution(exchange, ev.request_id), read,
        () => renderMcpQuestion(ev.server_name, ev.tool_name, ev.arguments_summary), add);
      return;
  }
}

function permissionPassages(
  requestId: string,
  resolved: { allowed: boolean; reason?: string } | undefined,
  read: PassageText,
  question: () => ComponentChildren,
  add: Add,
): void {
  const card = `.initiator-body .permission-body[data-request-id=${css(requestId)}]`;
  add({ selector: `${card} > .permission-text` }, INITIATOR, read.drawing(`permission:${requestId}`, question));
  const note = engineResolutionNote(resolved);
  if (note) add({ selector: `${card} > .permission-resolution-note` }, INITIATOR, note);
}

type StepEvent = Extract<ResponseEvent, { type: 'step' }>;

function responsePassages(exchange: Exchange, read: PassageText, add: Add): void {
  const events = exchangeResponseEvents(exchange, false, true);
  const shownWithoutDetails = headClampApplies(events, false) ? null : new Set(getCollapsedVisibleEvents(events));
  for (const event of events) {
    const details: Unfold = shownWithoutDetails !== null && !shownWithoutDetails.has(event) ? { details: true } : {};
    if (event.type === 'text') {
      if (!isMeaningfulText(event) || event.seq === undefined) continue;
      const seqs = (event.pieces ?? [{ seq: event.seq }]).flatMap((p) => (p.seq === undefined ? [] : [p.seq]));
      const selector = seqs.map((seq) => `.response-chunk[data-text-seq=${css(seq)}]`).join(', ');
      add({ selector }, details, read.markdown(event.md));
    } else if (event.type === 'step') {
      stepPassages(event, { ...details, steps: true }, add);
      for (const child of event.children ?? []) {
        stepPassages(child, { ...details, steps: true, subAgent: event.tool_use_id }, add);
      }
    } else if (event.seq !== undefined) {
      markerPassages(event, findRow(event.seq), add);
    }
  }
  const limit = exchangeEngineLimitDetail(exchange);
  if (limit) add({ selector: '.exchange-engine-limit > .authored-html' }, {}, read.markdown(limit));
  const error = exchangeError(exchange);
  // Every shape of the failure card opens on its message.
  if (error) add({ selector: '.exchange-error > p', nth: 0 }, {}, error.message);
  if (error?.agentExit) add({ selector: '.exchange-error-reason' }, {}, agentExitReason(error.agentExit));
}

const findRow = (seq: number) => `[data-find-row=${css(seq)}]`;

/** A transcript marker, drawn whatever the turn settings say. */
function markerPassages(event: ResponseEvent, row: string, add: Add): void {
  if (event.type === 'checkpoint') {
    add({ selector: `${row} .step-note-summary` }, {}, event.summary);
    add({ selector: `${row} .step-note-detail` }, {}, event.command);
  } else if (event.type === 'spoken_reply') {
    add({ selector: `${row} .spoken-reply-words` }, {}, event.text);
  } else if (event.type === 'event_wait') {
    add({ selector: `${row} .event-row-detail` }, {}, waitDetail(event.reason));
  }
}

function stepPassages(step: StepEvent, unfold: Unfold, add: Add): void {
  if (step.seq === undefined) return;
  const row = `${findRow(step.seq)} > .step-main`;
  const { description, detail } = stepRowText(step);
  add({ selector: `${row} > .step-description` }, unfold, description);
  add({ selector: `${row} > .step-detail` }, unfold, detail);
}

/** Each side question under the turn it follows: the latest one begun at or
 *  before it. One asked before every turn goes with the first. */
function sideQuestionsByTurn(exchanges: Exchange[], cards: readonly SideQuestion[]): Map<number, SideQuestion[]> {
  const byTurn = new Map<number, SideQuestion[]>();
  if (exchanges.length === 0) return byTurn;
  for (const card of cards) {
    let turn = 0;
    if (card.afterSeq !== null) {
      for (const [i, exchange] of exchanges.entries()) if (exchange.userSeq <= card.afterSeq) turn = i;
    } else {
      turn = exchanges.length - 1;
    }
    byTurn.set(turn, [...(byTurn.get(turn) ?? []), card]);
  }
  return byTurn;
}

function sideQuestionPassages(card: SideQuestion, read: PassageText, add: Add): void {
  const root = `.side-question-card[data-side-question-id=${css(card.id)}]`;
  const unfold: Unfold = card.dismissed ? { sideQuestion: card.id } : {};
  add({ selector: `${root} .side-question-question > .markdown-content`, inFeed: true }, unfold, read.markdown(card.question));
  if (card.status === 'answered') {
    add({ selector: `${root} .side-question-answer`, inFeed: true }, unfold, read.markdown(card.answer));
  } else if (card.status === 'failed') {
    add({ selector: `${root} .side-question-error`, inFeed: true }, unfold, card.error);
  }
}
