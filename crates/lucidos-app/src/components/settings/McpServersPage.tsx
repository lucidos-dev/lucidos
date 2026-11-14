import { useEffect, useRef, useState } from 'preact/hooks';
import type { VNode } from 'preact';
import {
  fetchMcpServers,
  getMcpAllowedTools,
  putMcpAllowedTools,
  removeMcpServer,
  setMcpAutoApprove,
  setMcpToolDisabled,
  startMcpServer,
  stopMcpServer,
  type McpServerStatus,
  type McpServersResponse,
  type McpToolStatus,
} from '../../api/client';
import { mcpServersVersion, showConfirm, showToast } from '../../store/store';
import { toFailed, type Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { Disclosure } from '../shared/Disclosure';
import { Explainer } from '../shared/Explainer';
import { ChevronDownIcon, ChevronRightIcon } from '../shared/icons';
import { LoadableError } from '../shared/LoadableError';
import { LoadableToggle } from '../shared/LoadableToggle';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkeletonProvider, SkText } from '../shared/Skeleton';
import { AllowlistEditor } from './AllowlistEditor';
import { intentsAfterServe, toolIntentKey, withToolIntents, type ToolIntents } from './mcpToolIntents';
import {
  describeMcpRowError,
  mcpHeaderSummary,
  mcpServerCostLine,
  mcpServerState,
  mcpServerStateLabel,
  mcpToolCostLine,
  type McpServerState,
  sortToolsByCost,
  type McpHeaderSummary,
  type McpRowError,
} from './mcpCost';

/** A per-server verb in flight. Start and stop are deliberately NOT optimistic:
 *  a start spawns a process and can fail for real reasons, so the row shows
 *  pending and settles on the response. */
export type McpRowVerb = 'start' | 'stop' | 'remove' | 'auto-approve';

const PENDING_LABEL: Partial<Record<McpRowVerb, string>> = {
  start: 'Starting...',
  stop: 'Stopping...',
  remove: 'Removing...',
};

export interface McpServerRowProps {
  /** Skeleton mode. Passed explicitly rather than read from the skeleton
   *  context, so the row stays a pure function and its states are assertable
   *  with no DOM. The Sk* leaves inside still read the context that
   *  `ListSkeletonOf` provides. */
  sk?: boolean;
  server?: McpServerStatus;
  expanded?: boolean;
  pending?: McpRowVerb | null;
  error?: McpRowError | null;
  onToggleExpanded?: () => void;
  onSetRunning?: (running: boolean) => void;
  onSetAutoApprove?: (autoApprove: boolean) => void;
  onRemove?: () => void;
  onSetToolDisabled?: (wireName: string, disabled: boolean) => void;
}

/** One tool under an expanded server: what the model is offered, what the
 *  server calls it where the two differ, and what it costs. */
function mcpToolRow(tool: McpToolStatus, props: McpServerRowProps): VNode {
  const wire = tool.wire_name;
  const state = wire === null ? 'uncallable' : tool.disabled ? 'disabled' : 'enabled';
  return (
    <div class="mcp-tool-row" key={wire ?? tool.name} data-state={state}>
      <div class="mcp-tool-info">
        <div class="mcp-tool-name">{wire ?? tool.name}</div>
        {wire !== null && wire !== tool.name && (
          <div class="mcp-tool-alias">The server calls it {tool.name}</div>
        )}
        {tool.description && <div class="mcp-tool-desc">{tool.description}</div>}
        <div class="mcp-tool-cost">{mcpToolCostLine(tool)}</div>
      </div>
      <div class="mcp-tool-actions">
        {/* No switch without a wire name: `disabled_tools` holds wire names, so
            there is nothing to store, and the tool is already uncallable. */}
        {wire !== null && (
          <LoadableToggle
            loaded
            checked={!tool.disabled}
            ariaLabel={`Offer ${wire} to the agent`}
            onChange={(on) => props.onSetToolDisabled?.(wire, !on)}
          />
        )}
      </div>
    </div>
  );
}

/** The shared label tone each server state wears. */
const STATE_TONE: Record<McpServerState, string> = {
  running: 'label-success',
  stopped: 'label-neutral',
  undispatchable: 'label-error',
};

/** One registered server, as a pure builder. `McpServersPage` calls it with
 *  real data. `ListSkeletonOf` calls it with `sk` inside a skeleton provider,
 *  so the placeholder IS this markup and cannot drift from it. */
export function mcpServerRow(props: McpServerRowProps): VNode {
  const { sk = false, server } = props;
  const state = server ? mcpServerState(server) : 'stopped';
  const pending = props.pending ?? null;
  const expanded = props.expanded ?? false;
  const toolCount = server?.tools.length ?? 0;
  const busy = pending !== null;
  // Start is refused for an id that cannot ride the wire, so the Run switch is
  // absent rather than present and failing. Auto-approve goes with it: nothing
  // on such a server is ever callable, so Remove is all that is left to offer.
  const showSwitches = sk || state !== 'undispatchable';

  return (
    // Keyed by id so a Remove patches the list rather than shuffling every row
    // below it. `ListSkeletonOf` overrides the key with the row index.
    <div class="mcp-server-block" key={server?.id} data-state={state}>
      <div class="list-row mcp-server-row">
        <div class="list-row-info">
          <div class="mcp-server-heading">
            <SkText class="title list-row-name" as="div" w="9rem">{server?.name}</SkText>
            <SkText class={`label ${STATE_TONE[state]}`} w="6rem">
              {server ? mcpServerStateLabel(server) : null}
            </SkText>
          </div>
          <SkText class="mcp-server-id" as="div" w="12rem">{server?.id}</SkText>
          <SkText class="mcp-server-cost" as="div" w="20rem">
            {server ? mcpServerCostLine(server) : null}
          </SkText>
          {!sk && state === 'undispatchable' && (
            <div class="mcp-server-note">
              This server's id cannot be used on the wire, so none of its tools can ever
              be called. Remove it, then register it again under a usable id.
            </div>
          )}
          {!sk && props.error && (
            <div class="mcp-row-error" data-kind={props.error.kind}>{props.error.message}</div>
          )}
          {(sk || toolCount > 0) && (
            <SkBlock w="6rem" h="1.25rem" round>
              <button
                type="button"
                class="settings-disclosure-toggle"
                aria-expanded={expanded}
                onClick={() => props.onToggleExpanded?.()}
              >
                {expanded ? <ChevronDownIcon size="1rem" /> : <ChevronRightIcon size="1rem" />}
                {toolCount === 1 ? '1 tool' : `${toolCount} tools`}
              </button>
            </SkBlock>
          )}
        </div>
        <div class="list-row-actions mcp-row-actions">
          {showSwitches && (
            <div class="mcp-switch">
              <SkText class="mcp-switch-label" w="4rem">
                {(pending && PENDING_LABEL[pending]) ?? 'Run'}
              </SkText>
              <SkBlock w="2.25rem" h="1.25rem" round>
                <LoadableToggle
                  loaded
                  checked={server?.running ?? false}
                  disabled={busy || !server}
                  ariaLabel={`Run ${server?.name ?? ''}`}
                  onChange={(on) => props.onSetRunning?.(on)}
                />
              </SkBlock>
            </div>
          )}
          {showSwitches && (
            <div class="mcp-switch">
              <SkText class="mcp-switch-label" w="5rem">Auto-approve</SkText>
              <SkBlock w="2.25rem" h="1.25rem" round>
                <LoadableToggle
                  loaded
                  checked={server?.auto_approve ?? false}
                  disabled={busy}
                  ariaLabel={`Auto-approve every tool on ${server?.name ?? ''}`}
                  onChange={(on) => props.onSetAutoApprove?.(on)}
                />
              </SkBlock>
            </div>
          )}
          <SkBlock w="4.5rem" h="2rem" round>
            <button
              type="button"
              class="action-btn action-btn-danger"
              disabled={busy}
              onClick={() => props.onRemove?.()}
            >
              Remove
            </button>
          </SkBlock>
        </div>
      </div>
      <Disclosure open={expanded && !sk && !!server}>
        {server && (
          <div class="mcp-tool-list">
            {sortToolsByCost(server.tools).map((tool) => mcpToolRow(tool, props))}
          </div>
        )}
      </Disclosure>
    </div>
  );
}

/** The cost figures, self-skeletonizing: the conditional lines are forced to
 *  appear in skeleton mode so the block does not grow as the data lands. */
function mcpCostSummary(summary: McpHeaderSummary | null, sk = false): VNode {
  return (
    <div class="mcp-cost-summary">
      <SkText class="mcp-cost-headline" as="div" w="20rem">{summary?.live}</SkText>
      {(sk || summary?.share) && (
        <SkText class="mcp-cost-share" as="div" w="14rem">{summary?.share}</SkText>
      )}
      {!sk && summary?.warning && <div class="mcp-cost-warning">{summary.warning}</div>}
      {(sk || summary?.running) && (
        <SkText class="mcp-cost-aside" as="div" w="17rem">{summary?.running}</SkText>
      )}
      {(sk || summary?.disabled) && (
        <SkText class="mcp-cost-aside" as="div" w="17rem">{summary?.disabled}</SkText>
      )}
    </div>
  );
}

export interface McpServersBodyProps {
  data: Loadable<McpServersResponse>;
  showLoading: boolean;
  expanded: ReadonlySet<string>;
  pending: Readonly<Record<string, McpRowVerb>>;
  errors: Readonly<Record<string, McpRowError>>;
  onToggleExpanded: (id: string) => void;
  onSetRunning: (server: McpServerStatus, running: boolean) => void;
  onSetAutoApprove: (server: McpServerStatus, autoApprove: boolean) => void;
  onRemove: (server: McpServerStatus) => void;
  onSetToolDisabled: (server: McpServerStatus, wireName: string, disabled: boolean) => void;
}

/** Everything above the allowlist editor, as a pure function of the `Loadable`.
 *  Exported so all four states are unit-testable without a DOM. */
export function mcpServersBody(props: McpServersBodyProps): VNode {
  const { data } = props;
  const summary =
    data.status === 'loaded'
      ? mcpHeaderSummary(
          data.data.totals,
          data.data.context_window,
          data.data.model,
          data.data.budget,
        )
      : null;

  return (
    <>
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mcp:cost">
          Context cost
          <Explainer title="Context cost">
            <p>
              A thread switches an MCP server on when it needs it. From then on,
              every tool the server offers rides on every request of that thread, as a
              name, a description and a JSON schema. A thread that never enables a
              server pays nothing for it.
            </p>
            <p>
              The headline is the worst case: one thread that enables every server.
              The engine measures it, so these figures are the same ones the request
              packer and the context viewer work from.
            </p>
          </Explainer>
        </div>
        {data.status === 'failed' ? (
          <LoadableError noun="MCP servers" error={data.error} />
        ) : (
          <LoadingFade
            showSkeleton={props.showLoading}
            skeleton={<SkeletonProvider>{mcpCostSummary(null, true)}</SkeletonProvider>}
          >
            {summary ? mcpCostSummary(summary) : null}
          </LoadingFade>
        )}
      </div>

      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mcp:servers">Servers</div>
        <p class="settings-row-note">
          Nothing starts an MCP server when the engine starts. A thread's next turn starts
          the servers it enabled, and an engine restart stops them all again.
        </p>
        {data.status === 'failed' ? null : (
          <LoadingFade
            showSkeleton={props.showLoading}
            skeleton={
              <ListSkeletonOf
                count={3}
                containerClass="list-rows mcp-server-list"
                row={() => mcpServerRow({ sk: true })}
              />
            }
          >
            {data.status === 'loaded' ? (
              data.data.servers.length === 0 ? (
                <div class="empty-state">
                  No MCP servers registered. Ask the Lucidos Agent to add one, and it
                  appears here with what it costs.
                </div>
              ) : (
                <div class="list-rows mcp-server-list">
                  {data.data.servers.map((server) =>
                    mcpServerRow({
                      server,
                      expanded: props.expanded.has(server.id),
                      pending: props.pending[server.id] ?? null,
                      error: props.errors[server.id] ?? null,
                      onToggleExpanded: () => props.onToggleExpanded(server.id),
                      onSetRunning: (running) => props.onSetRunning(server, running),
                      onSetAutoApprove: (on) => props.onSetAutoApprove(server, on),
                      onRemove: () => props.onRemove(server),
                      onSetToolDisabled: (wire, disabled) =>
                        props.onSetToolDisabled(server, wire, disabled),
                    }),
                  )}
                </div>
              )
            ) : null}
          </LoadingFade>
        )}
      </div>
    </>
  );
}

function withoutKey<T>(map: Record<string, T>, key: string): Record<string, T> {
  const next = { ...map };
  delete next[key];
  return next;
}

/** Settings → MCP Servers: what each registered server offers the agent, what
 *  it costs a thread that enables it, and the switches for both. */
export function McpServersPage() {
  const [served, setServed] = useState<Loadable<McpServersResponse>>({ status: 'not-loaded' });
  const [intents, setIntentState] = useState<ToolIntents>(new Map());
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set<string>());
  const [pending, setPending] = useState<Record<string, McpRowVerb>>({});
  const [errors, setErrors] = useState<Record<string, McpRowError>>({});
  const showLoading = useDelayedLoading(served);
  /** The pending switches, readable between awaits (ADR 0437). */
  const intentsNow = useRef<ToolIntents>(new Map());
  /** Every list fetch takes the next epoch as it starts. */
  const fetchEpoch = useRef(0);
  /** The epoch of the list on screen. An older fetch landing later is
   *  dropped, so a slow reply never replaces a newer one. */
  const servedEpoch = useRef(0);
  /** Tools whose switch request is out, by `toolIntentKey`. */
  const sending = useRef(new Set<string>());

  function setIntents(next: ToolIntents) {
    intentsNow.current = next;
    setIntentState(next);
  }

  /** Fetch the list, show it unless a newer fetch already landed, and end
   *  every switch it reflects. */
  async function load(): Promise<void> {
    const epoch = ++fetchEpoch.current;
    const data = await fetchMcpServers();
    if (epoch <= servedEpoch.current) return;
    servedEpoch.current = epoch;
    setServed({ status: 'loaded', data });
    setIntents(intentsAfterServe(intentsNow.current, epoch));
  }

  useEffect(() => {
    setServed({ status: 'loading' });
    // A newer fetch may have served the list while this one was out.
    load().catch((e) => { if (servedEpoch.current === 0) setServed(toFailed(e)); });
  }, []);

  /** Re-read the list after a mutation. Every figure on the page comes from one
   *  response, so the header and the rows can never disagree. A failure here
   *  toasts rather than blanking a list that is merely stale. */
  async function refresh(): Promise<void> {
    try {
      await load();
    } catch (e) {
      showToast(`Failed to refresh MCP servers: ${errorDetail(e)}`, 'error');
    }
  }

  // A server the agent registers, or a tool switched off on another device,
  // repaints this page with no reload (ADR 0118). Held, not dropped, while a
  // row verb runs, which refreshes when it settles. A pending switch needs no
  // guard: it is laid over whatever list lands.
  useVersionedRefresh(mcpServersVersion.value, Object.keys(pending).length > 0, () => void refresh());
  usePanelRefresh('MCP servers', Object.keys(pending).length > 0 ? null : refresh);

  async function runVerb(server: McpServerStatus, verb: McpRowVerb, call: () => Promise<void>) {
    setPending((prev) => ({ ...prev, [server.id]: verb }));
    setErrors((prev) => withoutKey(prev, server.id));
    try {
      await call();
      await refresh();
    } catch (e) {
      const described = describeMcpRowError(e);
      // A row that no longer exists cannot carry an error: reload instead, and
      // say what happened where the user will still see it.
      if (described.kind === 'gone') {
        showToast(described.message, 'error');
        await refresh();
      } else {
        setErrors((prev) => ({ ...prev, [server.id]: described }));
      }
    } finally {
      setPending((prev) => withoutKey(prev, server.id));
    }
  }

  function toggleExpanded(id: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }

  async function setRunning(server: McpServerStatus, running: boolean) {
    if (!running) {
      const confirmed = await showConfirm(
        `Stop the MCP server "${server.name}"?\n\nIt is switched off in every thread that enabled it. A thread that needs it again has to enable it again.`,
        'Stop',
        { variant: 'default' },
      );
      if (!confirmed) return;
    }
    await runVerb(server, running ? 'start' : 'stop', async () => {
      if (running) await startMcpServer(server.id);
      else await stopMcpServer(server.id);
    });
  }

  function setAutoApprove(server: McpServerStatus, autoApprove: boolean) {
    void runVerb(server, 'auto-approve', async () => {
      await setMcpAutoApprove(server.id, autoApprove);
    });
  }

  async function removeServer(server: McpServerStatus) {
    const confirmed = await showConfirm(
      `Remove the MCP server "${server.name}"?\n\nIts tools stop being offered to the Lucidos Agent, and the registration is deleted. Registering it again is the only way back.`,
      'Remove',
      { variant: 'danger' },
    );
    if (!confirmed) return;
    await runVerb(server, 'remove', async () => {
      await removeMcpServer(server.id);
    });
  }

  /** Optimistic, unlike start and stop: the switch is a local write with no
   *  process behind it. It is a pending intent laid over every list that
   *  lands until a list fetched after the engine's answer shows it (ADR 0437).
   *  Only the switch moves, never a figure: the refetch brings the engine's
   *  own totals. */
  function setToolDisabled(server: McpServerStatus, wireName: string, disabled: boolean) {
    const key = toolIntentKey(server.id, wireName);
    setIntents(new Map(intentsNow.current).set(key, { serverId: server.id, wireName, disabled, ackedAt: null }));
    if (!sending.current.has(key)) void sendSwitch(server, key);
  }

  /** Send a tool's latest switch, and send again if the user flipped it while
   *  the request was out. One request per tool at a time, because off then on
   *  for one tool does not commute. Switches on different tools do. */
  async function sendSwitch(server: McpServerStatus, key: string) {
    sending.current.add(key);
    try {
      for (let sent = intentsNow.current.get(key); sent; sent = intentsNow.current.get(key)) {
        await setMcpToolDisabled(server.id, sent.wireName, sent.disabled);
        const latest = intentsNow.current.get(key);
        if (latest && latest.disabled !== sent.disabled) continue;
        if (latest) setIntents(new Map(intentsNow.current).set(key, { ...latest, ackedAt: fetchEpoch.current }));
        break;
      }
    } catch (e) {
      const next = new Map(intentsNow.current);
      next.delete(key);
      setIntents(next);
      showToast(`Failed to update ${server.name}: ${errorDetail(e)}`, 'error');
    } finally {
      sending.current.delete(key);
    }
    await refresh();
  }

  const loadable: Loadable<McpServersResponse> = served.status === 'loaded'
    ? { status: 'loaded', data: withToolIntents(served.data, intents) }
    : served;

  return (
    <>
      {mcpServersBody({
        data: loadable,
        showLoading,
        expanded,
        pending,
        errors,
        onToggleExpanded: toggleExpanded,
        onSetRunning: (server, running) => void setRunning(server, running),
        onSetAutoApprove: setAutoApprove,
        onRemove: (server) => void removeServer(server),
        onSetToolDisabled: setToolDisabled,
      })}

      <AllowlistEditor
        title="MCP tool permissions"
        anchor="mcp:allowed-tools"
        noun="MCP tool permissions"
        placeholder="Mcp(slack:*)"
        load={getMcpAllowedTools}
        save={putMcpAllowedTools}
        description={
          <>
            <p>
              Tools the Lucidos Agent may call without asking. Patterns are{' '}
              <code>Mcp(&lt;server&gt;:&lt;tool&gt;)</code> for one tool, or{' '}
              <code>Mcp(&lt;server&gt;:*)</code> for a whole server.
            </p>
            <p>
              The <strong>Always allow</strong> buttons on an MCP permission card add
              entries here. The gate reads the file fresh on each prompt, so an edit
              applies to the next call.
            </p>
          </>
        }
      />
    </>
  );
}
