import type { ConnectionStatus } from '../store/types';

export type ConnectionNoticeStatus = ConnectionStatus | 'restarting' | 'switching-version';

function connectionTarget(workspace: string | null): string {
  return workspace ? `the ${workspace} engine` : 'the engine';
}

const CONNECTION_PHRASE: Record<ConnectionNoticeStatus, (target: string) => string> = {
  connected: (target) => `connected to ${target}`,
  connecting: (target) => `connecting to ${target}`,
  disconnected: (target) => `cannot reach ${target}`,
  stopped: (target) => `${target} is stopped`,
  starting: (target) => `${target} is starting`,
  restarting: (target) => `${target} is restarting`,
  'switching-version': (target) => `${target} is starting a new version`,
};

export function connectionPhrase(status: ConnectionNoticeStatus, workspace: string | null): string {
  return CONNECTION_PHRASE[status](connectionTarget(workspace));
}

export type NoticeLength = 'short' | 'full';

const RESTART_DETAIL = {
  consequence: 'Threads and messages will load in this window once it is back.',
  recovery: 'Waiting for it to come back.',
};

const CONNECTION_DETAIL: Partial<Record<ConnectionNoticeStatus, { consequence: string; recovery: string }>> = {
  connecting: {
    consequence: 'Threads and messages will not load or send in this window yet.',
    recovery: 'Waiting for an answer.',
  },
  disconnected: {
    consequence: 'Threads and messages will not load or send in this window.',
    recovery: 'Retrying every few seconds.',
  },
  stopped: {
    consequence: 'Threads and messages will not load or send in this window.',
    recovery: 'Start it to continue.',
  },
  starting: {
    consequence: 'Threads and messages will load in this window once it answers.',
    recovery: 'Waiting for it to boot.',
  },
  restarting: RESTART_DETAIL,
  'switching-version': RESTART_DETAIL,
};

export function isEngineUnreachable(status: ConnectionStatus): boolean {
  return status === 'disconnected' || status === 'stopped';
}

export function hasConnectionNotice(status: ConnectionNoticeStatus): boolean {
  return CONNECTION_DETAIL[status] !== undefined;
}

export function connectionNotice(
  status: ConnectionNoticeStatus,
  workspace: string | null,
  length: NoticeLength,
): { title: string; detail: string } | null {
  const detail = CONNECTION_DETAIL[status];
  if (!detail) return null;
  const phrase = connectionPhrase(status, workspace);
  return {
    title: phrase.charAt(0).toUpperCase() + phrase.slice(1),
    detail: length === 'full' ? `${detail.consequence} ${detail.recovery}` : detail.recovery,
  };
}

export function connectionNoticeSentence(
  status: ConnectionNoticeStatus,
  workspace: string | null,
  length: NoticeLength,
): string | null {
  const notice = connectionNotice(status, workspace, length);
  return notice ? `${notice.title}. ${notice.detail}` : null;
}
