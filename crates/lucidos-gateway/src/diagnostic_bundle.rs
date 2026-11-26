use chrono::{DateTime, SecondsFormat, Utc};
use lucidos_diagnostic_log::bundle::{
    Bundle, Manifest, Missing, Source, SourceStatus, TimelineLine, MAX_COMPRESSED_BYTES,
    MAX_EXPANDED_BYTES,
};
use lucidos_diagnostic_log::{
    snapshot::{Snapshot, FAILURE_WINDOW_DAYS, MAX_FAILURE_EVENTS},
    writer::{Record, FILE_NAME, ROTATED_FILE_NAME, SEGMENT_MAX_BYTES},
    Destination, CLIENT_LOG_DIR, CLIENT_RELAUNCH_LOG_DIR,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

const MAX_LINE_BYTES: u64 = 128 * 1024;

pub struct Sources {
    logs: Vec<(Source, Option<PathBuf>)>,
    workspace: PathBuf,
    pub boot_failure: Option<lucidos_diagnostic_log::Line>,
}
impl Sources {
    pub fn new(workspace: &Path, gateway_data: &Path, client_data: Option<PathBuf>) -> Self {
        let (client, relaunch) = client_data
            .as_deref()
            .map(|path| {
                (
                    path.join(CLIENT_LOG_DIR),
                    path.join(CLIENT_RELAUNCH_LOG_DIR),
                )
            })
            .unzip();
        Self {
            logs: vec![
                (
                    Source::Engine,
                    Some(workspace.join(".lucidos/diagnostic-log")),
                ),
                (
                    Source::Gateway,
                    Some(crate::diagnostic_log::dir_for(gateway_data)),
                ),
                (Source::Client, client),
                (Source::ClientRelaunch, relaunch),
            ],
            workspace: workspace.into(),
            boot_failure: None,
        }
    }
}

pub fn client_data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library/Application Support")
                .join(lucidos_installs::BUNDLE_IDENTIFIER)
        })
    } else {
        None
    }
}

fn push_record(bundle: &mut Bundle, source: Source, record: Record, destination: Destination) {
    let status = bundle.manifest.sources.entry(source).or_default();
    let Ok(ts) = DateTime::parse_from_rfc3339(&record.ts) else {
        status.invalid_lines += 1;
        return;
    };
    let message = record.render(destination);
    status.lines += 1;
    bundle.timeline.push(TimelineLine {
        ts: ts
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true),
        source,
        pid: record.pid,
        message,
    });
}

fn collect_log(bundle: &mut Bundle, source: Source, dir: Option<&Path>, destination: Destination) {
    bundle.manifest.sources.entry(source).or_default();
    let Some(dir) = dir else {
        bundle.manifest.sources.get_mut(&source).unwrap().missing = Some(Missing::Unsupported);
        return;
    };
    let mut present = false;
    for name in [ROTATED_FILE_NAME, FILE_NAME] {
        use std::os::unix::fs::OpenOptionsExt;
        let file = match File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(dir.join(name))
        {
            Ok(file)
                if file
                    .metadata()
                    .is_ok_and(|m| m.is_file() && m.len() <= SEGMENT_MAX_BYTES) =>
            {
                present = true;
                file
            }
            Ok(_) => {
                bundle
                    .manifest
                    .sources
                    .get_mut(&source)
                    .unwrap()
                    .unreadable_segments += 1;
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                bundle
                    .manifest
                    .sources
                    .get_mut(&source)
                    .unwrap()
                    .unreadable_segments += 1;
                continue;
            }
        };
        let mut reader = BufReader::new(file.take(SEGMENT_MAX_BYTES));
        loop {
            let mut bytes = Vec::new();
            match reader
                .by_ref()
                .take(MAX_LINE_BYTES + 1)
                .read_until(b'\n', &mut bytes)
            {
                Ok(0) => break,
                Ok(_) if bytes.len() as u64 > MAX_LINE_BYTES => {
                    bundle
                        .manifest
                        .sources
                        .get_mut(&source)
                        .unwrap()
                        .invalid_lines += 1;
                    bundle
                        .manifest
                        .sources
                        .get_mut(&source)
                        .unwrap()
                        .unreadable_segments += 1;
                    break;
                }
                Ok(_) => match serde_json::from_slice::<Record>(&bytes) {
                    Ok(record) => push_record(bundle, source, record, destination),
                    Err(_) => {
                        bundle
                            .manifest
                            .sources
                            .get_mut(&source)
                            .unwrap()
                            .invalid_lines += 1
                    }
                },
                Err(_) => {
                    bundle
                        .manifest
                        .sources
                        .get_mut(&source)
                        .unwrap()
                        .unreadable_segments += 1;
                    break;
                }
            }
        }
    }
    let status = bundle.manifest.sources.get_mut(&source).unwrap();
    if status.unreadable_segments > 0 {
        status.missing = Some(Missing::Unreadable);
    } else if !present {
        status.missing = Some(Missing::NotPresent);
    } else if status.invalid_lines > 0 {
        status.missing = Some(Missing::Invalid);
    }
}

pub fn build(
    sources: &Sources,
    snapshot: Option<Snapshot>,
    destination: Destination,
    cap: usize,
) -> Result<Vec<u8>, crate::BoxError> {
    let mut bundle = Bundle {
        manifest: Manifest {
            format_version: 1,
            sources: BTreeMap::new(),
            trimmed_lines: 0,
            gateway_version: env!("CARGO_PKG_VERSION").into(),
            gateway_build: crate::server::GATEWAY_BUILD_ID.into(),
            release: crate::LUCIDOS_RELEASE.into(),
            versions: BTreeMap::new(),
            missing_versions: BTreeMap::from([
                ("client".into(), Missing::NotRecorded),
                ("client-build".into(), Missing::NotRecorded),
                ("frontend".into(), Missing::NotRecorded),
                ("frontend-build".into(), Missing::NotRecorded),
            ]),
            failure_window_days: FAILURE_WINDOW_DAYS,
            failure_row_limit: MAX_FAILURE_EVENTS,
            configured: BTreeMap::new(),
            engine_healthy: None,
        },
        timeline: Vec::new(),
    };
    for (source, dir) in &sources.logs {
        collect_log(&mut bundle, *source, dir.as_deref(), destination);
    }
    for source in [
        Source::Events,
        Source::Faults,
        Source::EngineStatus,
        Source::Config,
    ] {
        bundle.manifest.sources.insert(
            source,
            SourceStatus {
                missing: snapshot.is_none().then_some(Missing::EngineUnavailable),
                ..Default::default()
            },
        );
    }
    if let Some(snapshot) = snapshot {
        for record in snapshot.events {
            push_record(&mut bundle, Source::Events, record, destination);
        }
        for record in snapshot.faults {
            push_record(&mut bundle, Source::Faults, record, destination);
        }
        bundle.manifest.versions = snapshot.versions;
        bundle.manifest.configured = snapshot.configured;
        bundle.manifest.engine_healthy = Some(snapshot.healthy);
    }
    if bundle.manifest.engine_healthy.is_none() {
        for version in ["engine", "engine-build"] {
            bundle
                .manifest
                .missing_versions
                .insert(version.into(), Missing::EngineUnavailable);
        }
    }
    collect_death(&mut bundle, &sources.workspace, destination);
    let boot = sources.boot_failure.as_ref();
    bundle.manifest.sources.insert(
        Source::BootFailure,
        SourceStatus {
            missing: boot.is_none().then_some(Missing::NotPresent),
            ..Default::default()
        },
    );
    if let Some(line) = boot {
        push_record(
            &mut bundle,
            Source::BootFailure,
            Record {
                ts: Utc::now().to_rfc3339(),
                pid: 0,
                label: String::new(),
                msg: String::new(),
                line: Some(line.clone()),
            },
            destination,
        );
    }
    bundle
        .timeline
        .sort_by(|a, b| a.ts.cmp(&b.ts).then(a.source.cmp(&b.source)));
    compress_to_cap(bundle, cap.min(MAX_COMPRESSED_BYTES))
}

fn compress_to_cap(bundle: Bundle, cap: usize) -> Result<Vec<u8>, crate::BoxError> {
    compress_to_limits(bundle, cap, MAX_EXPANDED_BYTES)
}

fn compress_to_limits(
    mut bundle: Bundle,
    cap: usize,
    expanded: usize,
) -> Result<Vec<u8>, crate::BoxError> {
    loop {
        let json = serde_json::to_vec(&bundle)?;
        if json.len() <= expanded {
            let bytes = zstd::stream::encode_all(json.as_slice(), 3)?;
            if bytes.len() <= cap {
                return Ok(bytes);
            }
        }
        if bundle.timeline.is_empty() {
            return Err("Diagnostic metadata exceeds bundle cap".into());
        }
        let trim = (bundle.timeline.len() / 8).max(1);
        bundle.timeline.drain(..trim);
        bundle.manifest.trimmed_lines += trim;
    }
}

fn collect_death(bundle: &mut Bundle, workspace: &Path, destination: Destination) {
    #[derive(Deserialize)]
    struct Death {
        old_pid: u32,
        exit_code: i32,
        died_at: DateTime<Utc>,
        supervisor_pid: u32,
    }
    let status = bundle
        .manifest
        .sources
        .entry(Source::LastDeath)
        .or_default();
    use std::os::unix::fs::OpenOptionsExt;
    let file = match File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(workspace.join(".lucidos/engine.last-death.json"))
    {
        Ok(file)
            if file
                .metadata()
                .is_ok_and(|m| m.is_file() && m.len() <= MAX_LINE_BYTES) =>
        {
            file
        }
        Ok(_) => {
            status.missing = Some(Missing::Invalid);
            return;
        }
        Err(e) => {
            status.missing = Some(if e.kind() == io::ErrorKind::NotFound {
                Missing::NotPresent
            } else {
                Missing::Unreadable
            });
            return;
        }
    };
    match serde_json::from_reader::<_, Death>(file.take(MAX_LINE_BYTES)) {
        Ok(death) => {
            let line = lucidos_diagnostic_log::line!(
                "engine {} exit {} supervisor {}",
                death.old_pid,
                death.exit_code,
                death.supervisor_pid
            );
            push_record(
                bundle,
                Source::LastDeath,
                Record {
                    ts: death.died_at.to_rfc3339(),
                    pid: death.old_pid,
                    label: String::new(),
                    msg: String::new(),
                    line: Some(line),
                },
                destination,
            );
        }
        Err(_) => status.missing = Some(Missing::Invalid),
    }
}

#[cfg(test)]
#[path = "diagnostic_bundle_tests.rs"]
mod tests;

#[allow(dead_code)]
pub async fn collect(
    state: &crate::server::GatewayState,
    workspace: &crate::registry::Workspace,
    destination: Destination,
) -> Result<Vec<u8>, crate::BoxError> {
    let mut sources = Sources::new(
        &workspace.resolve_dir(state.app_data()),
        state.app_data(),
        client_data_dir(),
    );
    if let Some(failure) = state.boot_failure(&workspace.id) {
        sources.boot_failure = Some(lucidos_diagnostic_log::line!(
            "boot failure terminal={} {}",
            failure.is_terminal(),
            lucidos_diagnostic_log::private(failure.message())
        ));
    }
    let address = state.engine_address(&workspace.id);
    let snapshot = match address {
        Some(address) => request_snapshot(&address, state.local_token()).await,
        None => None,
    };
    tokio::task::spawn_blocking(move || {
        build(&sources, snapshot, destination, MAX_COMPRESSED_BYTES)
    })
    .await?
}

async fn request_snapshot(
    address: &crate::engine_hop::EngineAddress,
    token: &str,
) -> Option<Snapshot> {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let request = Request::builder()
            .uri("/api/v1/engine/diagnostic-snapshot")
            .body(Body::empty())
            .expect("static snapshot request");
        let response = crate::engine_hop::send(
            address,
            request,
            crate::engine_hop::Credential::Local(token),
        )
        .await?;
        if !response.status().is_success() {
            return Err("Snapshot refused".into());
        }
        let bytes = to_bytes(Body::new(response.into_body()), 32 * 1024 * 1024).await?;
        Ok::<_, crate::BoxError>(serde_json::from_slice::<Snapshot>(&bytes)?)
    })
    .await;
    match result {
        Ok(Ok(snapshot)) => Some(snapshot),
        Ok(Err(_)) | Err(_) => None,
    }
}
