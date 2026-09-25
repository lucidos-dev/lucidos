import { describeFormRequest, openFormRequest } from '../../store/actions/form-requests';
import type { FormRequestOutcome } from '../../store/thread-events/thread-event-types';
import type { ResponseEvent } from '../../store/types';
import { eventRowBody } from './EventRow';
import type { EventRowTone } from './EventRow';

type FormRequestRowEvent = Extract<ResponseEvent, { type: 'form_request' }>;

/** The words and tint for each way a *form request* can stand. */
const FORM_REQUEST_STATE: Record<FormRequestOutcome | 'open', { label: string; tone: EventRowTone }> = {
  open: { label: 'Waiting for you', tone: 'live' },
  completed: { label: 'Done', tone: 'good' },
  canceled: { label: 'Canceled', tone: 'halted' },
  superseded: { label: 'Replaced', tone: 'lapsed' },
  expired: { label: 'Expired', tone: 'lapsed' },
  unknown: { label: 'Closed', tone: 'none' },
};

/** A form request in its turn: what the agent asked for, and how it stands.
 *
 *  While it is open it carries Open, which reopens the form. That is how a
 *  user reaches a request whose form they closed, or never saw because the
 *  stream frame was lost. Once resolved it is a plain record. */
export function FormRequestRow({ row, threadId }: { row: FormRequestRowEvent; threadId: string }) {
  const state = FORM_REQUEST_STATE[row.resolution ?? 'open'];
  const described = describeFormRequest(row.request);
  return eventRowBody({
    kind: 'form',
    mark: row.resolution ? 'arrived' : 'pending',
    state: row.resolution ?? 'open',
    role: 'form-request-row',
    subject: described.charAt(0).toUpperCase() + described.slice(1),
    stateLabel: state.label,
    tone: state.tone,
    action: row.resolution
      ? undefined
      : { label: 'Open', onClick: () => openFormRequest(threadId, row.request, { byUser: true }) },
  });
}
