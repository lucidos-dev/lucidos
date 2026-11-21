// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { BRIDGE_TYPE, BRIDGE_REPLY_TYPE } from '../../../../../packages/lucidos-sdk/src/_bridge';

vi.mock('../../utils/basePath', async (original) => ({
  ...(await original<typeof import('../../utils/basePath')>()),
  BASE_PATH: '/demo@remote',
  API: '/demo@remote/api/v1',
  WORKSPACE_ID: 'demo@remote',
}));

const { installAppBridge, _resetAppBridgeForTesting } = await import('./app-bridge');
let stop: () => void;
let frame: HTMLIFrameElement;
let fetchMock: ReturnType<typeof vi.fn>;
let reply: ReturnType<typeof vi.spyOn>;

beforeEach(() => {
  _resetAppBridgeForTesting();
  frame = document.createElement('iframe');
  frame.dataset.role = 'app-ui-frame';
  frame.src = '/demo@remote/app/habit-tracker/';
  document.body.appendChild(frame);
  reply = vi.spyOn(frame.contentWindow!, 'postMessage').mockImplementation(() => {});
  fetchMock = vi.fn(async () => new Response('{}'));
  vi.stubGlobal('fetch', fetchMock);
  stop = installAppBridge();
});
afterEach(() => { stop(); frame.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

function send(path: string, source: MessageEventSource | null = frame.contentWindow): void {
  const event = new MessageEvent('message', { data: {
    type: BRIDGE_TYPE, id: 'hostile-call', op: 'fetch', args: { path, method: 'GET' },
  } });
  Object.defineProperty(event, 'source', { value: source });
  window.dispatchEvent(event);
}

it('a legitimate app fetch stays at the selected remote workspace', async () => {
  send('/models');
  await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
  expect(fetchMock.mock.calls[0][0]).toBe('/demo@remote/api/v1/models');
});

it.each([
  '/../../~/api/v1/control/workspaces', '/data/../../../../local/api/v1/threads/list',
  '/data/%2e%2e/%2e%2e/%2e%2e/sibling@other/api/v1/threads/list',
  '//localhost/~/api/v1/control/workspaces', 'https://example.com/api/v1/models',
  '/~/api/v1/control/workspaces', '/local/api/v1/models',
])('refuses a frame attempt to escape through %s', async (path) => {
  send(path);
  await vi.waitFor(() => expect(reply).toHaveBeenCalled());
  expect(reply.mock.calls[0][0]).toMatchObject({ type: BRIDGE_REPLY_TYPE, ok: false });
  expect(fetchMock).not.toHaveBeenCalled();
});

it('an unmounted or nested sender cannot use a mounted app bridge', async () => {
  send('/models', window);
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(fetchMock).not.toHaveBeenCalled();
  expect(reply).not.toHaveBeenCalled();
});
