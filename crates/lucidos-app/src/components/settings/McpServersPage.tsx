import { useEffect, useRef, useState } from 'preact/hooks';
import type { VNode } from 'preact';
import {
  fetchMcpServers,
  getMcpAllowedTools,
  putMcpAllowedTools,
  removeMcpServer,
  setMcpAutoApprove,
  setMcpDisabledTools,
  startMcpServer,
  stopMcpServer,
  type McpServerStatus,
  type McpServersResponse,
  type McpToolStatus,
} from '../../api/client';
import { showConfirm, showToast } from '../../store/store';
import { toFailed, type Loadable } from '../../store/types';
import { errorDetail } from '../../utils/errorDetail';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { Explainer } from '../shared/Explainer';
import { ChevronDownIcon, ChevronRightIcon } from '../shared/icons';
import { LoadableError } from '../shared/LoadableError';
import { LoadableToggle } from '../shared/LoadableToggle';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkeletonProvider, SkText } from '../shared/Skeleton';
import { AllowlistEditor } from './AllowlistEditor';
import {
  describeMcpRowError,
  mcpHeaderSummary,
  mcpServerCostLine,
  mcpServerState,
  mcpServerStateLabel,
  mcpToolCostLine,
  disabledWireNames,
  patchToolDisabled,
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
            there is nothing to store, and the tool is already uncallable.
            Held while a server verb runs, because that verb reloads the list
            and a flip made under it would be reverted by the reply. */}
        {wire !== null && (
          <LoadableToggle
            loaded
            checked={!tool.disabled}
            disabled={props.pending != null}
            ariaLabel={`Offer ${wire} to the agent`}
            onChange={(on) => props.onSetToolDisabled?.(wire, !on)}
          />
        )}
      </div>
    </div>
  );
}

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
            <SkText class="mcp-state-chip" w="6rem">
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
                class="mcp-tools-toggle"
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
      {expanded && !sk && server && (
        <div class="mcp-tool-list">
          {sortToolsByCost(server.tools).map((tool) => mcpToolRow(tool, props))}
        </div>
      )}
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
      {(sk || summary?.stopped) && (
        <SkText class="mcp-cost-aside" as="div" w="17rem">{summary?.stopped}</SkText>
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
      ? mcpHeaderSummary(data.data.totals, data.data.context_window, data.data.model)
      : null;

  return (
    <>
      <div class="settings-section">
        <div class="settings-section-title" data-search-anchor="mcp:cost">
          Context cost
          <Explainer title="Context cost">
            <p>
              Every tool an MCP server offers rides on every request, as a name, a
              description and a JSON schema. That is a permanent per-turn cost, paid
              whether or not the agent uses the tool.
            </p>
            <p>
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
          Nothing starts an MCP server when the engine starts. Every server running here
          was switched on by hand, and an engine restart switches them all off again.
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
 *  it costs every request, and the switches for both. */
export function McpServersPage() {
  const [loadable, setLoadable] = useState<Loadable<McpServersResponse>>({ status: 'not-loaded' });
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set<string>());
  const [pending, setPending] = useState<Record<string, McpRowVerb>>({});
  const [errors, setErrors] = useState<Record<string, McpRowError>>({});
  const showLoading = useDelayedLoading(loadable);
  /** What the page is showing, readable synchronously. A second tool click in
   *  the same frame must see the first click's flip, and the render carrying
   *  it has not happened yet. */
  const shown = useRef<McpServersResponse | null>(null);
  /** The last queued write per server. The disabled-tools PUT replaces the
   *  whole set, so two in flight can settle backwards and re-enable a tool the
   *  user just switched off. */
  const writes = useRef(new Map<string, Promise<void>>());

  function showData(data: McpServersResponse) {
    shown.current = data;
    setLoadable({ status: 'loaded', data });
  }

  useEffect(() => {
    setLoadable({ status: 'loading' });
    fetchMcpServers()
      .then(showData)
      .catch((e) => setLoadable(toFailed(e)));
  }, []);

  /** Re-read the list after a mutation. Every figure on the page comes from one
   *  response, so the header and the rows can never disagree. A failure here
   *  toasts rather than blanking a list that is merely stale.
   *
   *  `stillWanted` is checked once the data lands, so a reload can be dropped
   *  by whatever happened while it was in flight. Without it a tool switched
   *  off mid-reload bounces back: the reply predates the flip and knows
   *  nothing about it. */
  async function refresh(stillWanted: () => boolean = () => true) {
    try {
      const data = await fetchMcpServers();
      if (stillWanted()) showData(data);
    } catch (e) {
      showToast(`Failed to refresh MCP servers: ${errorDetail(e)}`, 'error');
    }
  }

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

  function setRunning(server: McpServerStatus, running: boolean) {
    void runVerb(server, running ? 'start' : 'stop', async () => {
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
   *  process behind it. Only the switch moves, never a figure. The refetch
   *  brings the engine's own totals, which is what makes the saving visible.
   *
   *  Writes for one server are queued rather than raced, and each sends the set
   *  the page is showing when its turn comes. Two clicks therefore agree on the
   *  same final set, however their round trips interleave. Only the last write
   *  in the queue refetches, so a run of clicks costs one reload. */
  function setToolDisabled(server: McpServerStatus, wireName: string, disabled: boolean) {
    const base = shown.current;
    if (!base) return;
    showData(patchToolDisabled(base, server.id, wireName, disabled));

    const isLast = () => writes.current.get(server.id) === mine;
    const run = async () => {
      const current = shown.current?.servers.find((s) => s.id === server.id);
      if (!current) return;
      try {
        await setMcpDisabledTools(server.id, disabledWireNames(current));
      } catch (e) {
        showToast(`Failed to update ${server.name}: ${errorDetail(e)}`, 'error');
      }
      // Either way, once this is the last queued write: on success for the real
      // figures, on failure to undo the flip.
      if (isLast()) await refresh(isLast);
    };
    const prior = writes.current.get(server.id) ?? Promise.resolve();
    // Run on either settle, so one rejected write cannot strand the queue.
    const mine: Promise<void> = prior.then(run, run);
    writes.current.set(server.id, mine);
  }

  return (
    <>
      {mcpServersBody({
        data: loadable,
        showLoading,
        expanded,
        pending,
        errors,
        onToggleExpanded: toggleExpanded,
        onSetRunning: setRunning,
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
