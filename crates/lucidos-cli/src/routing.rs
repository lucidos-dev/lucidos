//! Closed reads and spawns through the caller's own engine, never a peer URL.

use crate::workspace::{BoxError, Workspace};
use crate::{EventsCmd, ThreadsCmd};
use clap::Args;
use lucidos_gateway_stamp::workspace_operation::{
    ReadKind, ReadRequest, RoutingRequest, WorkspaceOperation, PROTOCOL_HEADER, PROTOCOL_VERSION,
};
use serde_json::Value;

#[derive(Args, Debug, Clone, Default)]
pub(crate) struct Selection {
    /// Routing gateway from `workspaces targets`; defaults to the source gateway.
    #[arg(long, global = true)]
    pub via: Option<String>,
    /// Target workspace, optionally qualified as workspace@remote.
    #[arg(long, global = true)]
    pub to: Option<String>,
}

impl Selection {
    pub fn active(&self) -> bool {
        self.via.is_some() || self.to.is_some()
    }

    fn for_read(&self, mut read: ReadRequest) -> Result<(Self, ReadRequest), BoxError> {
        let mut route = self.clone();
        if let Some(link) = read
            .thread_id
            .as_deref()
            .and_then(|thread| thread.strip_prefix("thread:"))
        {
            let (workspace, thread) = link
                .split_once('/')
                .ok_or("A thread link must name its workspace and thread.")?;
            if route
                .to
                .as_ref()
                .is_some_and(|to| to != workspace && to.split('@').next() != Some(workspace))
            {
                return Err("The thread link and --to select different workspaces.".into());
            }
            route.to.get_or_insert_with(|| workspace.to_string());
            read.thread_id = Some(thread.to_string());
        }
        if route.active() && route.to.is_none() && read.kind != ReadKind::WorkspaceTargets {
            return Err("Select --to <workspace[@remote]> with --via. Use workspaces targets to discover targets.".into());
        }
        read.path()?;
        Ok((route, read))
    }
}

fn request(
    kind: ReadKind,
    thread: Option<String>,
    params: Vec<(&'static str, String)>,
) -> ReadRequest {
    let mut query = params
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect::<std::collections::BTreeMap<_, _>>();
    query.remove("thread_id");
    ReadRequest {
        kind,
        thread_id: thread,
        query,
    }
}

pub(crate) fn operation(
    ws: &Workspace,
    route: &Selection,
    operation: WorkspaceOperation,
) -> Result<Value, BoxError> {
    if !crate::http::has_origin_token() {
        return Err("Routed operations require a Lucidos-spawned thread's origin token. Keep the caller's source-engine environment; --via never replaces it.".into());
    }
    let url = format!("{}/api/v1/workspace-operations", ws.base_url());
    let response = crate::http::client()?
        .post(&url)
        .json(&RoutingRequest {
            via: route.via.clone(),
            to: route.to.clone(),
            operation,
        })
        .send()
        .map_err(|_| {
            "The routed operation did not finish. Its outcome may be uncertain; it was not retried."
        })?;
    let supported = response
        .headers()
        .get(PROTOCOL_HEADER)
        .is_some_and(|version| version == PROTOCOL_VERSION);
    if let Some(note) = crate::http::child_reports_note(response.headers()) {
        eprintln!("{note}");
    }
    let status = response.status();
    let text = response
        .text()
        .map_err(|_| "The operation reply could not be read. The operation was not retried.")?;
    if !supported && (status.is_success() || matches!(status.as_u16(), 404 | 405)) {
        return Err(
            "Upgrade the caller's source workspace engine to use routed operations.".into(),
        );
    }
    if !status.is_success() {
        let outcome = if status.is_client_error() {
            "was refused"
        } else {
            "failed; its outcome may be uncertain and it was not retried"
        };
        return Err(format!("Routed operation {outcome}: HTTP {status}: {text}").into());
    }
    serde_json::from_str(&text).map_err(|_| "The selected workspace returned invalid JSON.".into())
}

pub(crate) fn read_value(
    ws: &Workspace,
    route: &Selection,
    read: ReadRequest,
) -> Result<Value, BoxError> {
    let (route, read) = route.for_read(read)?;
    if route.active() || read.kind == ReadKind::WorkspaceTargets {
        return operation(ws, &route, WorkspaceOperation::Read { request: read });
    }
    let url = format!("{}{}", ws.base_url(), read.path()?);
    let text = crate::http::send_expect_success(
        "GET",
        &url,
        crate::http::client()?.get(&url).query(&read.query_pairs()?),
    )?;
    Ok(serde_json::from_str(&text)?)
}

fn print_read(ws: &Workspace, route: &Selection, read: ReadRequest) -> Result<(), BoxError> {
    println!("{}", serde_json::to_string(&read_value(ws, route, read)?)?);
    Ok(())
}

pub(crate) fn targets(ws: &Workspace, via: Option<String>) -> Result<(), BoxError> {
    print_read(
        ws,
        &Selection { via, to: None },
        request(ReadKind::WorkspaceTargets, None, Vec::new()),
    )
}

fn liveness(ws: &Workspace, route: &Selection, thread: Option<&str>) {
    let Some(thread) = thread else {
        return;
    };
    // An unavailable liveness read claims no state, matching ordinary event reads.
    if let Ok(value) = read_value(
        ws,
        route,
        request(ReadKind::ThreadLiveness, Some(thread.into()), Vec::new()),
    ) {
        if let Some(line) = crate::events::liveness_header_line(&value) {
            eprintln!("{line}");
        }
    }
}

pub(crate) fn threads(
    ws: &Workspace,
    route: &Selection,
    action: &ThreadsCmd,
) -> Result<(), BoxError> {
    let read = match action {
        ThreadsCmd::Get { thread } | ThreadsCmd::Messages { thread } | ThreadsCmd::Liveness { thread } => {
            let kind = match action { ThreadsCmd::Get { .. } => ReadKind::ThreadSummary, ThreadsCmd::Messages { .. } => ReadKind::ThreadMessages, _ => ReadKind::ThreadLiveness };
            if kind != ReadKind::ThreadLiveness { liveness(ws, route, Some(thread)); }
            request(kind, Some(thread.clone()), Vec::new())
        }
        ThreadsCmd::List { active, status, source, parent, my_children, has_draft, change_state, .. }
        | ThreadsCmd::Count { active, status, source, parent, my_children, has_draft, change_state } => {
            if *my_children { return Err("Routed reads need an explicit --parent in the target workspace, not --my-children from the source.".into()); }
            let (kind, limit) = match action { ThreadsCmd::List { limit, .. } => (ReadKind::Threads, *limit), _ => (ReadKind::ThreadsCount, None) };
            request(kind, None, crate::threads::build_query_params(&crate::threads::ListFilters {
                active: active.then_some(true), status, source: source.as_deref(), limit, parent: parent.clone(),
                has_draft: *has_draft, change_state: change_state.clone(),
            }))
        }
        _ => return Err("Routing supports thread get, messages, liveness, list and count. Other thread operations stay on the source workspace.".into()),
    };
    print_read(ws, route, read)
}

pub(crate) fn events(
    ws: &Workspace,
    route: &Selection,
    action: &EventsCmd,
) -> Result<(), BoxError> {
    let read = match action {
        EventsCmd::Query { event_type, since, until, before_event_id, after_event_id, thread_id, limit } => {
            liveness(ws, route, thread_id.as_deref());
            request(ReadKind::Events, thread_id.clone(), crate::events::build_query_params(&crate::events::QueryFilters {
                event_type: event_type.as_deref(), since: since.as_deref(), until: until.as_deref(),
                before_event_id: before_event_id.as_deref(), after_event_id: after_event_id.as_deref(), thread_id: thread_id.as_deref(), limit: *limit,
            }))
        }
        EventsCmd::Count { event_type, since, until, thread_id } => {
            liveness(ws, route, thread_id.as_deref());
            request(ReadKind::EventsCount, thread_id.clone(), crate::events::build_count_params(&crate::events::CountFilters {
                event_type: event_type.as_deref(), since: since.as_deref(), until: until.as_deref(), thread_id: thread_id.as_deref(),
            }))
        }
        EventsCmd::Emit { .. } => return Err("Remote event emission is not a routed operation. Routing permits event reads and counts only.".into()),
    };
    print_read(ws, route, read)
}

pub(crate) fn ladder(
    ws: &Workspace,
    route: &Selection,
    surface: Option<crate::CliLadderSurface>,
    thread: Option<String>,
    json: bool,
) -> Result<(), BoxError> {
    if route.active() && surface.is_none() && thread.is_none() {
        return Err("A routed ladder read needs --thread <target thread> or --surface <surface>; the source thread is never substituted.".into());
    }
    let params = surface
        .map(|surface| vec![("surface", surface.as_wire().to_string())])
        .unwrap_or_default();
    let body = read_value(
        ws,
        route,
        request(ReadKind::IntelligenceLadder, thread, params),
    )?;
    crate::intelligence_ladder::print_body(body, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn read_selectors_parse_after_the_subcommand_without_changing_the_source() {
        let cli = crate::Cli::try_parse_from([
            "lucidos",
            "threads",
            "messages",
            "--via",
            "routing-gateway",
            "--to",
            "target@cloud",
            "--thread",
            "thread-1",
        ])
        .unwrap();
        let crate::Command::Threads {
            routing,
            action: ThreadsCmd::Messages { thread },
        } = cli.command
        else {
            panic!()
        };
        assert_eq!(routing.via.as_deref(), Some("routing-gateway"));
        assert_eq!(routing.to.as_deref(), Some("target@cloud"));
        assert_eq!(thread, "thread-1");
    }
    #[test]
    fn links_supply_names_but_cannot_override_an_explicit_target() {
        let route = Selection {
            via: Some("routing-gateway".into()),
            to: Some("target@cloud".into()),
        };
        let read = request(
            ReadKind::ThreadMessages,
            Some("thread:other/thread-1".into()),
            Vec::new(),
        );
        assert!(route.for_read(read).is_err());
        let read = request(
            ReadKind::ThreadMessages,
            Some("thread:target/thread-1".into()),
            Vec::new(),
        );
        let (route, read) = route.for_read(read).unwrap();
        assert_eq!(route.to.as_deref(), Some("target@cloud"));
        assert_eq!(read.thread_id.as_deref(), Some("thread-1"));
    }
}
