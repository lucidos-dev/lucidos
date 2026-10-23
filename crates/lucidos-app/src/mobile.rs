//! Mobile access for the packaged desktop app: surface the connect URLs and
//! drive the Tailscale setup the user consents to in the UI. The whole picture
//! is [`system-knowhow/remote-access.md`](../../../system-knowhow/remote-access.md).
//!
//! `tailscale serve` (tailnet-private), NEVER `tailscale funnel` (public):
//! Lucidos has **no inbound API auth**. Workspace engines stay behind the
//! gateway on loopback-only ports.
//!
//! **Nothing here runs on the main thread.** Every command is an `async fn`
//! whose body runs through [`tauri::async_runtime::spawn_blocking`]. Tauri runs
//! a synchronous command on the main thread, and all three of these block.
//!
//! **Reading state never runs the CLI.** Tailnet membership and the MagicDNS
//! name come from `lucidos-tailscale`, so a Mac with Tailscale working but no
//! CLI still gets an accurate description of itself. Only actions are gated.
//!
//! **Exit 0 is not proof.** The GUI executable exits 0 while printing an error,
//! so the CLI probe demands a parseable version and every action re-reads the
//! world afterwards.

use serde::Serialize;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

use crate::desktop::engine_port;

/// Bound on the MagicDNS reverse lookup. The resolver is local (`100.100.100.100`)
/// whenever we get this far, so this is a stall guard, not a budget.
const REVERSE_DNS_TIMEOUT: Duration = Duration::from_millis(1500);

/// Bound on the "is anything serving HTTPS" probe. Loopback-speed in practice.
const SERVE_PROBE_TIMEOUT: Duration = Duration::from_millis(700);

/// Bound on `tailscale version`, the one CLI call the status path makes. Answers
/// in milliseconds when healthy; the ceiling exists for when it does not.
const CLI_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Bound on one `ipconfig getifaddr` call. It answers instantly on a healthy
/// machine. The ceiling is here because this runs on the path a settings pane
/// awaits, and every other call on that path is already bounded.
const LAN_IP_TIMEOUT: Duration = Duration::from_secs(2);

/// The port `tailscale serve` fronts, and the one [`tailscale_serve`] configures.
const TAILNET_HTTPS_PORT: u16 = 443;

/// Bound on `tailscale serve` **before** it tells us it is waiting for the
/// tailnet. Configuring a mapping is quick, so this is a stall guard.
const SERVE_CONFIGURE_TIMEOUT: Duration = Duration::from_secs(20);

/// Bound on `tailscale serve` **after** it has printed the tailnet-approval URL.
///
/// Once that notice appears the command is no longer stalled. It is waiting for
/// a human to approve in a browser, and completes by itself when they do. So
/// the deadline stops being a stall guard and becomes a patience budget: ten
/// minutes is long enough to find the right login, and short enough that an
/// abandoned run does not sit there forever. Cancel is the real escape hatch.
const SERVE_APPROVAL_TIMEOUT: Duration = Duration::from_secs(600);

/// Bound on the wait for something to answer HTTPS after `serve` returns. A
/// freshly written mapping needs a moment, and a single probe turned that into
/// "reported success but nothing is answering".
const SERVE_HTTPS_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the supervising loops re-check. Short enough to feel immediate on a
/// cancel, long enough not to spin a core.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long an exited child's output readers get to reach EOF before their
/// buffer is read. Scheduling latency, not work, so this is a wide margin over
/// what it actually takes. See [`supervise_serve`] for why it exists at all.
const DRAIN_SETTLE: Duration = Duration::from_millis(500);

/// Where the connect URLs point. `lan_ip` and the Tailscale fields are `None`
/// when not detectable.
///
/// The LAN *URL* is deliberately not pre-built here. Whether a LAN address is
/// reachable at all depends on the gateway's network bind. The frontend reads
/// that from `GET /api/v1/network-config` and combines it with `lan_ip` and
/// `port`.
#[derive(Serialize)]
pub struct ConnectInfo {
    /// The stable gateway port the URLs use.
    pub port: u16,
    pub localhost_url: String,
    pub lan_ip: Option<String>,
    pub tailscale: TailscaleInfo,
}

/// What this Mac's Tailscale setup looks like, as two independent facts.
///
/// **Tailnet state** is read without a CLI. **CLI availability** gates the
/// buttons and nothing else. Keeping them apart is what lets the page stay
/// accurate on a Mac that has Tailscale working but no CLI installed.
#[derive(Serialize)]
pub struct TailscaleInfo {
    /// Tailscale is present at all: the app bundle, or a CLI. Drives the
    /// "Get Tailscale" offer, so it deliberately does NOT mean "usable".
    pub installed: bool,
    /// This Mac holds a tailnet address, so it is signed in and connected.
    pub on_tailnet: bool,
    /// The tailnet IPv4, when on a tailnet. Reachable over plain HTTP from any
    /// device on the same tailnet, which is Route A in `remote-access.md`.
    pub tailnet_ip: Option<String>,
    /// MagicDNS name, e.g. `mymac.tailnet-name.ts.net` (no scheme). `None` on a
    /// tailnet with MagicDNS disabled, which is not the same as being offline.
    pub magic_dns_name: Option<String>,
    /// `https://<magic_dns_name>`, set **only** once something is proven to be
    /// serving it. Before `tailscale serve` runs nothing listens on 443, so
    /// publishing the URL earlier would advertise a dead address.
    pub serve_url: Option<String>,
    /// A working `tailscale` CLI was found. Gates the actions only: never the
    /// reporting above.
    pub cli_available: bool,
}

/// Surface localhost / LAN / Tailscale connect URLs (mirrors the dev
/// `show_banner` in `scripts/lib/workspace.sh`).
///
/// Off the main thread even though it is only a few seconds of probes: those
/// seconds are the whole time the Mobile Access pane spends opening, and on the
/// main thread they are seconds the window cannot paint.
#[tauri::command]
pub async fn get_connect_info() -> Result<ConnectInfo, String> {
    off_main_thread("get_connect_info", connect_info).await
}

fn connect_info() -> ConnectInfo {
    let port = engine_port();
    ConnectInfo {
        port,
        localhost_url: format!("http://localhost:{port}"),
        lan_ip: detect_lan_ip(),
        tailscale: tailscale_status(),
    }
}

/// Run a blocking body on a worker thread, so the calling command never occupies
/// the main thread or an async-runtime worker.
///
/// One helper rather than three copies of the same `spawn_blocking` and
/// join-error mapping, and one place where the rule is stated.
async fn off_main_thread<T, F>(label: &'static str, work: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("{label} could not run: {e}"))
}

/// Detect this Mac's tailnet state and whether a CLI is available. Folded into
/// [`get_connect_info`] (which the Mobile Access page refetches), so it's an
/// internal helper rather than its own command.
fn tailscale_status() -> TailscaleInfo {
    let cli = tailscale_cli();
    let tailnet_addr = lucidos_tailscale::tailnet_ipv4();
    let magic_dns_name =
        tailnet_addr.and_then(|ip| lucidos_tailscale::magic_dns_name(ip, REVERSE_DNS_TIMEOUT));

    // Only a name we can show is answering becomes a URL. See `serve_is_live`.
    let serve_url = magic_dns_name
        .as_ref()
        .filter(|_| tailnet_addr.is_some_and(serve_is_live))
        .map(|h| format!("https://{h}"));

    TailscaleInfo {
        // The app bundle counts even without a CLI: offering "Get Tailscale" to
        // someone who already has it would be the wrong instruction.
        installed: cli.is_some() || std::path::Path::new(TAILSCALE_APP_BUNDLE).exists(),
        on_tailnet: tailnet_addr.is_some(),
        tailnet_ip: tailnet_addr.map(|ip| ip.to_string()),
        magic_dns_name,
        serve_url,
        cli_available: cli.is_some(),
    }
}

/// The macOS GUI app bundle. Its presence means Tailscale is installed; its
/// executable is NOT a CLI and is never run (see the module docs and
/// `lucidos_tailscale::TAILSCALE_CANDIDATES`).
const TAILSCALE_APP_BUNDLE: &str = "/Applications/Tailscale.app";

/// Is the address we are about to publish actually answering?
///
/// Deliberately a bounded TCP connect to **443 on the tailnet address**, and
/// deliberately NOT `tailscale serve status`. Asking the CLI whether *a* serve
/// mapping exists answers the wrong question: `remote-access.md` documents a
/// second gateway served on 8443, and a config containing only that mapping is
/// non-empty while `https://<name>` on 443 stays dead. The UI publishes exactly
/// one URL, so the only honest test is whether exactly that endpoint responds.
///
/// It proves a listener, not a working certificate. Tailscale can be listening
/// while a first-run cert is still provisioning, so the serve run says as much
/// in its own failure message.
fn serve_is_live(addr: Ipv4Addr) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from((addr, TAILNET_HTTPS_PORT)),
        SERVE_PROBE_TIMEOUT,
    )
    .is_ok()
}

/// Run a command with a hard deadline, killing it if it overruns.
///
/// `std::process::Command::output()` waits forever, and every caller is driven
/// by a button or a settings pane, so none can afford that. A wedged `tailscaled`
/// or a hung `ipconfig` would otherwise leave the pane loading with nothing to
/// show and no way to know why.
///
/// The two failures are kept apart in the message: "the binary is not there"
/// and "it never answered" call for different things from the reader.
///
/// Only safe for commands with SMALL output. It waits for exit before reading
/// the pipes, so a child that filled a pipe buffer would block instead of
/// exiting. The `serve` run does NOT use this, because it must read the
/// child's output while it is still running (see [`supervise_serve`]).
fn output_with_timeout(
    mut cmd: Command,
    label: &str,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run {label}: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| format!("failed to read {label} output: {e}"))
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{label} did not finish within {}s and was stopped",
                    timeout.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("failed to wait for {label}: {e}")),
        }
    }
}

/// The Mobile Access actions that must not overlap, held as Tauri managed state.
///
/// Both guards are load-bearing now that the commands run off the main thread.
/// Two `tailscale serve` children racing to write the same mapping is not
/// something to find out about from a bug report.
///
/// The shapes differ because the actions do: a serve run is cancellable and so
/// hands out a flag, while `up` is an interactive browser login with nothing to
/// cancel from here.
#[derive(Clone, Default)]
pub struct MobileAccessRuns {
    /// Cancellation flag of the in-flight `serve` run, when one holds the slot.
    serve: ServeCancelSlot,
    /// Set while a `tailscale up` is in flight.
    up: Arc<AtomicBool>,
}

/// The one serve run's cancellation flag, or `None` when the slot is free.
type ServeCancelSlot = Arc<Mutex<Option<Arc<AtomicBool>>>>;

/// What a caller gets when it claims the serve slot: the flag its own run must
/// watch, and a release on drop so no early return can strand the slot.
struct ServeSlot {
    cancel: Arc<AtomicBool>,
    slot: ServeCancelSlot,
}

impl Drop for ServeSlot {
    fn drop(&mut self) {
        *lock(&self.slot) = None;
    }
}

/// Take a lock without caring whether a panicking thread poisoned it. Nothing
/// under these locks can be left half-written: they hold a flag and an option.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl MobileAccessRuns {
    /// Claim the serve slot, or `None` when a run already holds it.
    fn start_serve(&self) -> Option<ServeSlot> {
        let mut slot = lock(&self.serve);
        if slot.is_some() {
            return None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *slot = Some(Arc::clone(&cancel));
        Some(ServeSlot {
            cancel,
            slot: Arc::clone(&self.serve),
        })
    }

    /// Ask the in-flight serve run to stop. No-op when nothing is running.
    fn cancel_serve(&self) {
        if let Some(flag) = lock(&self.serve).as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Claim the `up` slot, or `None` when a sign-in is already in flight.
    fn start_up(&self) -> Option<UpSlot> {
        self.up
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| UpSlot {
                held: Arc::clone(&self.up),
            })
    }
}

/// The `up` slot, released on drop for the same reason [`ServeSlot`] is. A slot
/// released by an explicit call at the end of the happy path is one a panic or
/// an early return strands forever. A stranded slot disables the Sign in button
/// for the rest of the process.
struct UpSlot {
    held: Arc<AtomicBool>,
}

impl Drop for UpSlot {
    fn drop(&mut self) {
        self.held.store(false, Ordering::SeqCst);
    }
}

/// Bring this machine onto a tailnet (`tailscale up`). This is interactive: the
/// CLI opens a browser for the one-time tailnet login (or accepts a
/// pre-authorized auth key). Returns once the command completes; the caller
/// re-reads [`tailscale_status`] to see the result.
#[tauri::command]
pub async fn tailscale_up(
    auth_key: Option<String>,
    runs: State<'_, MobileAccessRuns>,
) -> Result<(), String> {
    let runs = runs.inner().clone();
    off_main_thread("tailscale up", move || {
        // Held for the call, released on drop.
        let _slot = runs.start_up().ok_or(ALREADY_SIGNING_IN)?;
        run_tailscale_up(auth_key.as_deref())
    })
    .await?
}

fn run_tailscale_up(auth_key: Option<&str>) -> Result<(), String> {
    let cli = tailscale_cli().ok_or_else(|| NO_CLI.to_string())?;
    let mut cmd = Command::new(&cli);
    cmd.arg("up");
    if let Some(key) = auth_key.map(str::trim).filter(|k| !k.is_empty()) {
        cmd.arg(format!("--auth-key={key}"));
    }
    run_checked(cmd, "tailscale up")?;
    // Post-condition, not ceremony: the reported bug was `up` "succeeding" and
    // changing nothing, which the page could only render as a reload.
    if lucidos_tailscale::tailnet_ipv4().is_none() {
        return Err(
            "tailscale up reported success but this Mac still has no tailnet address. \
             Open the Tailscale app and check it is signed in."
                .to_string(),
        );
    }
    Ok(())
}

/// What to say when an action needs a CLI and there is none. Names the two ways
/// to get one, since the user plainly has Tailscale working without it.
const NO_CLI: &str = "The Tailscale command-line tool isn't available. \
     Install it from the Tailscale app (Install CLI), or with `brew install tailscale`.";

/// What a second Expose press is told while the first is still running.
const ALREADY_EXPOSING: &str =
    "Tailscale setup is already running. Watch the badge, or cancel it first.";

/// The same, for a second Sign in press.
const ALREADY_SIGNING_IN: &str =
    "A Tailscale sign-in is already running. Finish it in the browser Tailscale opened.";

// --- Exposing the engine over the tailnet ---

/// The Tauri event carrying [`ServePhase`] frames while an Expose run is in
/// flight. The name must match `TAILSCALE_SERVE_PROGRESS_EVENT` in
/// `src/utils/tauri.ts`.
const SERVE_PROGRESS_EVENT: &str = "tailscale-serve-progress";

/// Where an Expose run currently is.
///
/// Internally tagged on `phase`, mirrored by a TypeScript discriminated union.
/// A missing arm is therefore a `tsc` error rather than a blank line in the
/// toast. Same shape as the packaged updater's `AppUpdatePhase`.
///
/// No variant carries a fraction, and that is deliberate: not one step of this
/// flow can honestly report one, so the surface shows a spinner. A fabricated
/// bar would be worse than no bar.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "kebab-case")]
enum ServePhase {
    /// Claimed the slot, about to look for a CLI.
    Starting,
    /// Reading the tailnet address and the MagicDNS name.
    CheckingTailnet,
    /// `tailscale serve` is running.
    Configuring,
    /// The CLI says Serve is not enabled on this tailnet and is waiting for
    /// someone to approve it in a browser. `url` is the link **it printed**, and
    /// the toast offers it as a button. The run keeps waiting and finishes by
    /// itself once the approval lands.
    AwaitingTailnetApproval {
        url: String,
    },
    /// The mapping is configured; waiting for something to answer on 443.
    WaitingForHttps,
    /// Serving, and proven to be.
    Done {
        url: String,
    },
    Failed {
        message: String,
    },
    Cancelled,
}

fn emit_phase(app: &AppHandle, phase: ServePhase) {
    // Best-effort by construction: an emit that fails costs the narration, not
    // the run, and the command's own resolution still reports the outcome.
    let _ = app.emit(SERVE_PROGRESS_EVENT, phase);
}

/// How an Expose run ended, when it did not succeed. Cancelled is kept apart
/// from failed all the way out, because the page must not report a deliberate
/// stop as an error.
enum ServeFailure {
    Cancelled,
    Failed(String),
}

/// Expose the engine over the tailnet at `https://<machine>.<tailnet>.ts.net`
/// with an auto-renewed cert (`tailscale serve`). Tailnet-private, NOT `funnel`
/// (the engine has no inbound auth). Returns the connect URL on success.
///
/// Progress arrives out-of-band on [`SERVE_PROGRESS_EVENT`]; this future says
/// nothing until it is over. Awaiting it silently is what made the button look
/// dead for twenty seconds.
#[tauri::command]
pub async fn tailscale_serve(
    app: AppHandle,
    runs: State<'_, MobileAccessRuns>,
) -> Result<String, String> {
    let runs = runs.inner().clone();
    off_main_thread("tailscale serve", move || serve_blocking(&app, &runs)).await?
}

/// Abandon the in-flight Expose run. The outcome arrives as a `cancelled` frame,
/// not as this future's resolution. A no-op when nothing is running, which is
/// the right answer for a button the user pressed as the run was ending anyway.
#[tauri::command]
pub async fn cancel_tailscale_serve(runs: State<'_, MobileAccessRuns>) -> Result<(), String> {
    runs.cancel_serve();
    Ok(())
}

/// One Expose run, start to finish, with the slot held for its whole life.
fn serve_blocking(app: &AppHandle, runs: &MobileAccessRuns) -> Result<String, String> {
    let Some(slot) = runs.start_serve() else {
        // Deliberately emits nothing: a refused second press must not overwrite
        // the running run's narration with its own terminal frame.
        return Err(ALREADY_EXPOSING.to_string());
    };
    let outcome = serve_run(&slot.cancel, &mut |phase| emit_phase(app, phase));
    match &outcome {
        Ok(url) => emit_phase(app, ServePhase::Done { url: url.clone() }),
        Err(ServeFailure::Cancelled) => emit_phase(app, ServePhase::Cancelled),
        Err(ServeFailure::Failed(message)) => emit_phase(
            app,
            ServePhase::Failed {
                message: message.clone(),
            },
        ),
    }
    outcome.map_err(|failure| match failure {
        ServeFailure::Cancelled => "Tailscale setup was cancelled.".to_string(),
        ServeFailure::Failed(message) => message,
    })
}

/// The run itself, with the phase sink passed in so nothing below this line
/// needs an `AppHandle`.
fn serve_run(
    cancel: &AtomicBool,
    on_phase: &mut dyn FnMut(ServePhase),
) -> Result<String, ServeFailure> {
    on_phase(ServePhase::Starting);
    let cli = tailscale_cli().ok_or_else(|| ServeFailure::Failed(NO_CLI.to_string()))?;

    on_phase(ServePhase::CheckingTailnet);
    let addr = lucidos_tailscale::tailnet_ipv4().ok_or_else(|| {
        ServeFailure::Failed(
            "This Mac is not on a tailnet yet. Sign in to Tailscale first.".to_string(),
        )
    })?;
    let name = lucidos_tailscale::magic_dns_name(addr, REVERSE_DNS_TIMEOUT).ok_or_else(|| {
        ServeFailure::Failed(
            "This Mac is on a tailnet but has no MagicDNS name, so there is no HTTPS \
             address to serve. Enable MagicDNS for your tailnet and try again."
                .to_string(),
        )
    })?;
    let port = engine_port();

    // Proxy the tailnet HTTPS endpoint at the root path to the loopback engine,
    // in the background (the mapping survives this process).
    on_phase(ServePhase::Configuring);
    let (current, legacy) = serve_arg_forms(port);
    match run_serve_attempt(&cli, &current, cancel, on_phase) {
        Ok(()) => {}
        // ONLY a rejected flag earns a retry under the older syntax. Everything
        // else ends here with its own reason. See `ServeAttemptError`.
        Err(ServeAttemptError::FlagRejected(current_err)) => {
            if let Err(legacy_err) = run_serve_attempt(&cli, &legacy, cancel, on_phase) {
                return Err(match legacy_err {
                    ServeAttemptError::Cancelled => ServeFailure::Cancelled,
                    ServeAttemptError::Failed(e) | ServeAttemptError::FlagRejected(e) => {
                        ServeFailure::Failed(both_serve_forms_failed(&current_err, &e))
                    }
                });
            }
        }
        Err(other) => return Err(other.into()),
    }

    // Same post-condition rule as `up`: only report the URL we can show is live.
    // Polled rather than probed once, because a mapping written a moment ago is
    // allowed a moment to come up.
    on_phase(ServePhase::WaitingForHttps);
    if !wait_until(|| serve_is_live(addr), cancel, SERVE_HTTPS_TIMEOUT) {
        if cancel.load(Ordering::SeqCst) {
            return Err(ServeFailure::Cancelled);
        }
        return Err(ServeFailure::Failed(format!(
            "tailscale serve was configured but nothing answered on {name} within {}s. \
             If this is the first time, the certificate may still be provisioning; \
             `tailscale serve status` shows what was configured.",
            SERVE_HTTPS_TIMEOUT.as_secs()
        )));
    }
    Ok(format!("https://{name}"))
}

/// Poll `probe` until it answers true, the deadline passes, or a cancel lands.
/// Split out so the loop's shape is testable without a tailnet.
fn wait_until(mut probe: impl FnMut() -> bool, cancel: &AtomicBool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        if probe() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The two `tailscale serve` invocations we know, **current first**. The syntax
/// changed in CLI 1.52 and the old form has since been removed;
/// `system-knowhow/remote-access.md` has both forms and what each CLI answers.
///
/// The argv is deliberately **literal**, never derived from `serve --help` at
/// runtime. `serve` is tailnet-private while `funnel` is the open internet,
/// they are adjacent subcommands, and the engine has **no inbound API auth**.
/// So the one thing this must never do is guess. Version drift is absorbed by
/// trying a second known-good form, never by inventing one.
///
/// `--https` is pinned to [`TAILNET_HTTPS_PORT`] rather than left to the CLI's
/// default, because that is exactly the port [`serve_is_live`] then probes. The
/// target carries no path, so it mounts at `/` under both forms.
///
/// There is deliberately **no `--yes`**: it suppresses interactive prompts, and
/// what this waits on is a tailnet policy a human enables in a browser.
///
/// The fallback is a temporary measure, registered in
/// `docs/temporary-measures.md` § 3. It ages out once a pre-1.52 CLI stops
/// being worth supporting.
fn serve_arg_forms(port: u16) -> (Vec<String>, Vec<String>) {
    let target = format!("http://127.0.0.1:{port}");
    let current = vec![
        "serve".to_string(),
        "--bg".to_string(),
        format!("--https={TAILNET_HTTPS_PORT}"),
        target.clone(),
    ];
    let legacy = vec![
        "serve".to_string(),
        "https".to_string(),
        "/".to_string(),
        target,
    ];
    (current, legacy)
}

/// Both syntaxes were rejected, so say so without picking a favourite.
///
/// Reporting only the current form's error would hide the actionable one from
/// exactly the installs the fallback exists for. On a pre-1.52 CLI the current
/// form fails as an unknown flag whatever else is wrong. A real fault behind it
/// is then only ever named by the legacy attempt, and reporting only the legacy
/// error has the mirror problem.
///
/// So both are kept, current first, with the second labelled as a retry under
/// the older syntax. Reachable only when the current form was **rejected as a
/// flag**, which is what stops this appearing on a modern CLI that simply timed
/// out.
fn both_serve_forms_failed(current_err: &str, legacy_err: &str) -> String {
    if current_err == legacy_err {
        return current_err.to_string();
    }
    format!(
        "{current_err}\n\nRetried with the older `serve https /` syntax, which failed with: \
         {legacy_err}"
    )
}

/// Why one `serve` attempt did not succeed.
///
/// The split exists to hold one rule: **only [`Self::FlagRejected`] earns a
/// retry under the older syntax.** A deadline or a cancel is not the CLI
/// declining our argv. Retrying on either puts an irrelevant syntax error on
/// top of the failure the user actually hit.
enum ServeAttemptError {
    /// The CLI exited non-zero saying it does not know a flag we passed. On a
    /// pre-1.52 CLI that is `--bg` / `--https=`, and the positional form is
    /// worth a try.
    FlagRejected(String),
    /// Any other non-zero exit, a deadline, or a failure to spawn at all.
    Failed(String),
    Cancelled,
}

impl From<ServeAttemptError> for ServeFailure {
    fn from(e: ServeAttemptError) -> Self {
        match e {
            ServeAttemptError::Cancelled => Self::Cancelled,
            ServeAttemptError::Failed(m) | ServeAttemptError::FlagRejected(m) => Self::Failed(m),
        }
    }
}

/// One `serve` attempt, supervised. A `Command` is not reusable once run and not
/// `Clone`, so each form builds its own.
fn run_serve_attempt(
    cli: &str,
    args: &[String],
    cancel: &AtomicBool,
    on_phase: &mut dyn FnMut(ServePhase),
) -> Result<(), ServeAttemptError> {
    let mut cmd = Command::new(cli);
    cmd.args(args)
        // Nothing may ever block on a terminal we do not have. A GUI-spawned
        // child reading a prompt is a hang, so the input is simply closed.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| ServeAttemptError::Failed(format!("failed to run tailscale serve: {e}")))?;
    supervise_serve(
        child,
        cancel,
        SERVE_CONFIGURE_TIMEOUT,
        SERVE_APPROVAL_TIMEOUT,
        on_phase,
    )
}

/// Watch a running `serve` child: stream what it says, notice when it starts
/// waiting on the tailnet, and hold it to a deadline. Which deadline depends on
/// which of those two things it is doing.
///
/// **Streaming is the load-bearing part.** On a tailnet without Serve enabled
/// the CLI prints an approval URL and then blocks, polling the control plane
/// until a human visits it. Reading the pipes only after exit threw that line
/// away with the killed child.
///
/// Two reader threads therefore append into a shared buffer while this loop
/// polls exit, cancel and the deadline. The moment the approval notice appears
/// the phase changes and the deadline re-bases on the approval budget, since
/// the child is waiting rather than stalled.
///
/// **On exit, the readers are given a moment to finish first.** `try_wait` can
/// see a child terminate before its reader has appended the last of what it
/// wrote, and the classification below reads that buffer. A pre-1.52 CLI
/// rejecting `--bg` loses exactly that race, and an empty buffer would skip the
/// fallback that exists for it. Bounded by [`DRAIN_SETTLE`]. The kill paths do
/// NOT wait, since the notice they care about arrived long before.
fn supervise_serve(
    mut child: Child,
    cancel: &AtomicBool,
    configure_timeout: Duration,
    approval_timeout: Duration,
    on_phase: &mut dyn FnMut(ServePhase),
) -> Result<(), ServeAttemptError> {
    let seen = Arc::new(Mutex::new(String::new()));
    // Each reader holds a clone of the sender and nothing is ever sent, so the
    // channel disconnects exactly when the last reader ends. That makes one
    // `recv_timeout` a bounded "wait for both readers, or give up".
    let (readers_done, readers_ended) = std::sync::mpsc::channel::<()>();
    if let Some(out) = child.stdout.take() {
        drain_into(out, Arc::clone(&seen), readers_done.clone());
    }
    if let Some(err) = child.stderr.take() {
        drain_into(err, Arc::clone(&seen), readers_done.clone());
    }
    drop(readers_done);

    let mut deadline = Instant::now() + configure_timeout;
    let mut approval: Option<String> = None;
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ServeAttemptError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // Let the readers reach EOF before reading what they collected.
                let _ = readers_ended.recv_timeout(DRAIN_SETTLE);
                let output = readable_output(&seen);
                if status.success() {
                    return Ok(());
                }
                let message = if output.is_empty() {
                    format!("tailscale serve failed ({status})")
                } else {
                    format!("tailscale serve failed: {output}")
                };
                return Err(if is_flag_rejection(&output) {
                    ServeAttemptError::FlagRejected(message)
                } else {
                    ServeAttemptError::Failed(message)
                });
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ServeAttemptError::Failed(format!(
                    "failed to wait for tailscale serve: {e}"
                )));
            }
            Ok(None) => {}
        }
        if approval.is_none() {
            // Searched against the raw buffer, under the lock, with no copy: this
            // runs on every tick and the skew warning carries no URL to confuse it.
            let found = tailnet_approval_url(&lock(&seen));
            if let Some(url) = found {
                approval = Some(url.clone());
                deadline = Instant::now() + approval_timeout;
                on_phase(ServePhase::AwaitingTailnetApproval { url });
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ServeAttemptError::Failed(timed_out_message(
                approval.as_deref(),
                if approval.is_some() {
                    approval_timeout
                } else {
                    configure_timeout
                },
                &readable_output(&seen),
            )));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Copy a child pipe into the shared buffer as the bytes arrive.
///
/// Chunk-wise rather than line-wise on purpose: a line read only completes at a
/// newline. The whole point is to see what a child about to be killed has
/// already said. `from_utf8_lossy` cannot panic on a chunk boundary.
///
/// `done` is never sent on. It exists so that dropping it at EOF disconnects the
/// channel, which is how [`supervise_serve`] waits for every reader at once.
fn drain_into(
    mut pipe: impl Read + Send + 'static,
    sink: Arc<Mutex<String>>,
    done: std::sync::mpsc::Sender<()>,
) {
    std::thread::spawn(move || {
        let mut chunk = [0u8; 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => lock(&sink).push_str(&String::from_utf8_lossy(&chunk[..n])),
            }
        }
        drop(done);
    });
}

fn readable_output(seen: &Mutex<String>) -> String {
    without_version_skew_warning(&lock(seen))
}

/// What to say when an attempt ran out of time.
///
/// The approval case is the one that matters, and it gets its own sentence. The
/// user is not looking at a stall but at a run that was waiting for them, and
/// the link is the whole answer.
fn timed_out_message(approval_url: Option<&str>, waited: Duration, output: &str) -> String {
    if let Some(url) = approval_url {
        return format!(
            "Still waiting for Serve to be enabled on your tailnet after {}s, so setup was \
             stopped. Enable it at {url} and press Expose again.",
            waited.as_secs()
        );
    }
    let mut message = format!(
        "tailscale serve did not finish within {}s and was stopped.",
        waited.as_secs()
    );
    if !output.is_empty() {
        message.push_str("\n\nIt had said:\n");
        message.push_str(output);
    }
    message
}

/// The tailnet-approval URL a `serve` child printed, if it printed one.
///
/// Verbatim from the CLI, never composed here: the node id in the query string
/// is not something we could reconstruct. The URL is checked before it is
/// handed on, because this is subprocess output the page opens in the system
/// browser. See [`is_tailscale_https_url`].
fn tailnet_approval_url(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|token| token.trim_end_matches(['.', ',', ')', '"', '\'']))
        .find(|token| is_tailscale_https_url(token))
        .map(str::to_string)
}

/// Is this a URL we are willing to open: HTTPS, on a Tailscale host?
///
/// Both halves are required. A plain-HTTP link would be a downgrade we should
/// not perform on the user's behalf. The host check stops anything that gets a
/// line into this output from choosing the destination.
///
/// The userinfo split takes the segment AFTER the last `@`, which is the real
/// host, so `https://login.tailscale.com@evil.example/` is rejected rather than
/// mistaken for Tailscale.
fn is_tailscale_https_url(candidate: &str) -> bool {
    let Some(rest) = candidate.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "tailscale.com" || host.ends_with(".tailscale.com")
}

/// Did the CLI reject our argv? The **only** reason to retry the older syntax.
///
/// Go's `flag` package is what a pre-1.52 CLI answers `--bg` / `--https=` with,
/// so these are its words. Deliberately narrow: anything this does not match
/// ends the run with its own reason rather than collecting a second, irrelevant
/// one.
fn is_flag_rejection(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    ["flag provided but not defined", "flag needs an argument"]
        .iter()
        .any(|marker| lowered.contains(marker))
        || lowered.contains("unknown flag")
        || lowered.contains("unknown shorthand flag")
}

/// Drop Tailscale's client/daemon version-skew warning.
///
/// Printed on stderr by **every** command whenever the CLI and the running
/// `tailscaled` differ in version. That is the normal state of a Homebrew CLI
/// beside the Mac App Store daemon. Streaming stderr would otherwise make it
/// the first line of every message we show, ahead of the actual reason.
fn without_version_skew_warning(output: &str) -> String {
    output
        .lines()
        .filter(|line| !line.trim_start().starts_with("Warning: client version"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Locate a **working** `tailscale` CLI, or `None`.
///
/// Resolution is `lucidos-tailscale`'s: an env override, then absolute paths,
/// then the bare name for a shell that has a `PATH`. The old list here led with
/// `/Applications/Tailscale.app/Contents/MacOS/Tailscale`, the GUI executable,
/// which existed on every Mac with Tailscale and shadowed the real CLI beside
/// it.
///
/// The probe then demands a **parseable version**, not merely a zero exit,
/// because that GUI binary exits 0 while printing an error. Anything that
/// cannot say what version it is does not get to be our CLI.
#[cfg(target_os = "macos")]
fn tailscale_cli() -> Option<String> {
    let bin = lucidos_tailscale::tailscale_binary();
    // Bounded: this runs on every Mobile Access load, and a wedged CLI or
    // daemon would otherwise hang the pane on its loading state forever.
    let mut probe = Command::new(&bin);
    probe.arg("version");
    let out = output_with_timeout(probe, "tailscale version", CLI_PROBE_TIMEOUT).ok()?;
    if !out.status.success() {
        return None;
    }
    reports_a_version(&String::from_utf8_lossy(&out.stdout)).then_some(bin)
}

/// Does this `tailscale version` output come from something that actually is
/// the CLI?
///
/// Pure so the exit-0 liar can be pinned by a test rather than by a Mac. Real
/// output opens with the version itself (`1.96.4-t41cb72f27`); the GUI
/// executable opens with an apology and exits 0 all the same.
fn reports_a_version(stdout: &str) -> bool {
    let first = stdout.lines().next().unwrap_or_default().trim();
    let major = first.split('.').next().unwrap_or_default();
    !major.is_empty() && major.chars().all(|c| c.is_ascii_digit())
}

#[cfg(not(target_os = "macos"))]
fn tailscale_cli() -> Option<String> {
    None
}

/// Best LAN IPv4 for this machine (`ipconfig getifaddr en0`, then `en1`).
/// macOS-only; `None` elsewhere or when offline.
#[cfg(target_os = "macos")]
fn detect_lan_ip() -> Option<String> {
    for iface in ["en0", "en1"] {
        let mut cmd = Command::new("ipconfig");
        cmd.args(["getifaddr", iface]);
        // Bounded like every other call on this path: `get_connect_info` is what
        // a settings pane awaits, and an unbounded spawn here was the last way
        // left to hang it.
        let Ok(out) = output_with_timeout(cmd, "ipconfig getifaddr", LAN_IP_TIMEOUT) else {
            continue;
        };
        if out.status.success() {
            let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !ip.is_empty() {
                return Some(ip);
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn detect_lan_ip() -> Option<String> {
    None
}

/// Turn a finished command into `Ok`, or a readable error carrying its stderr.
fn check_status(out: std::process::Output, label: &str) -> Result<(), String> {
    if out.status.success() {
        return Ok(());
    }
    let stderr = without_version_skew_warning(&String::from_utf8_lossy(&out.stderr));
    Err(if stderr.is_empty() {
        format!("{label} failed ({})", out.status)
    } else {
        format!("{label} failed: {stderr}")
    })
}

/// Run a command to completion with **no deadline**, mapping a non-zero exit or
/// a spawn failure to a readable error.
///
/// The missing deadline is deliberate and belongs to `up` alone: signing in is
/// interactive, so it legitimately takes as long as the user takes. Do not
/// "fix" this by bounding it. Blocking is safe because the caller runs on a
/// worker thread, and a double press is already refused by
/// [`MobileAccessRuns::start_up`].
fn run_checked(mut cmd: Command, label: &str) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run {label}: {e}"))?;
    check_status(out, label)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from `tailscale serve --bg --https=443 http://127.0.0.1:5252` on
    /// a tailnet with Serve not enabled, CLI 1.96.4 against a 1.98.9 daemon. The
    /// command printed this and then blocked indefinitely.
    const SERVE_NOT_ENABLED: &str = "Warning: client version \"1.96.4-t41cb72f27\" != tailscaled server version \"1.98.9-t4fb758c39-g200941d74\"\n\nServe is not enabled on your tailnet.\nTo enable, visit:\n\n         https://login.tailscale.com/f/serve?node=nodeidEXAMPLE1234\n";

    fn sh(script: &str) -> Child {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the stub")
    }

    fn no_phases() -> impl FnMut(ServePhase) {
        |_| {}
    }

    #[test]
    fn the_macos_gui_executable_is_not_accepted_as_a_cli() {
        // Verbatim from /Applications/Tailscale.app/Contents/MacOS/Tailscale run
        // under the packaged environment. It goes to STDOUT and the process
        // EXITS 0, so an exit-code check alone reads it as a working CLI. That
        // is exactly how Mobile Access came to show a Sign in button that
        // silently did nothing on a Mac already on its tailnet.
        assert!(!reports_a_version(
            "The Tailscale GUI failed to start: The operation couldn't be completed. \
             (Tailscale.CLIError error 3.)"
        ));
    }

    #[test]
    fn real_tailscale_version_output_is_accepted() {
        // `tailscale version` leads with the version and then adds build lines.
        assert!(reports_a_version(
            "1.96.4-t41cb72f27\n  go version: go1.24.0\n"
        ));
        assert!(reports_a_version("1.98.9\n"));
    }

    #[test]
    fn a_hung_probe_is_killed_at_the_deadline() {
        // `get_connect_info` is awaited by a settings pane, so an unbounded
        // `.output()` on a wedged CLI leaves it loading forever with nothing to
        // show. `sleep` stands in for that: the call must give up, not block.
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("30");
        let start = Instant::now();
        let err = output_with_timeout(cmd, "sleepy", Duration::from_millis(200))
            .expect_err("the deadline must fire");
        // A timeout must not read as a missing binary: the same guard bounds the
        // version probe and `ipconfig`, where "never answered" and "not
        // installed" send the reader somewhere completely different.
        assert!(err.contains("did not finish within"), "{err}");
        assert!(err.contains("sleepy"), "{err}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "took {:?}, so the deadline did not fire",
            start.elapsed()
        );
    }

    #[test]
    fn a_missing_binary_is_not_reported_as_a_timeout() {
        let cmd = Command::new("/nonexistent/tailscale");
        let err = output_with_timeout(cmd, "tailscale serve", Duration::from_secs(5))
            .expect_err("a missing binary must fail");
        assert!(err.starts_with("failed to run tailscale serve:"), "{err}");
    }

    #[test]
    fn a_prompt_command_still_returns_its_output() {
        let mut cmd = Command::new("/bin/echo");
        cmd.arg("1.96.4");
        let out = output_with_timeout(cmd, "echo", Duration::from_secs(5)).expect("echo answers");
        assert!(out.status.success());
        assert!(reports_a_version(&String::from_utf8_lossy(&out.stdout)));
    }

    #[test]
    fn empty_or_wordy_output_is_not_a_version() {
        assert!(!reports_a_version(""));
        assert!(!reports_a_version("\n"));
        assert!(!reports_a_version("command not found"));
        // A leading blank line is not a version either: the version must be the
        // first thing said, as it is in every real build.
        assert!(!reports_a_version("\n1.96.4"));
    }

    #[test]
    fn the_current_serve_form_is_the_flag_syntax() {
        // What `tailscale serve --help` documents on 1.52+, and what
        // `system-knowhow/remote-access.md` tells a human to run by hand. The
        // port in `--https=` must be the one `serve_is_live` probes, or a
        // success would be reported against an endpoint nothing is fronting.
        let (current, _) = serve_arg_forms(5252);
        assert_eq!(
            current,
            ["serve", "--bg", "--https=443", "http://127.0.0.1:5252"]
        );
        assert_eq!(TAILNET_HTTPS_PORT, 443);
    }

    #[test]
    fn no_serve_form_passes_yes() {
        // `--yes` looks like the fix for a command that hangs, and is not: it
        // suppresses interactive prompts, while what this waits on is a tailnet
        // policy a human enables in a browser. Measured: with `--yes` the
        // command blocks identically.
        let (current, legacy) = serve_arg_forms(5252);
        for form in [&current, &legacy] {
            assert!(!form.iter().any(|a| a == "--yes"), "{form:?}");
        }
    }

    #[test]
    fn the_legacy_serve_form_is_the_pre_rework_syntax_without_bg() {
        // The fallback only ever runs on a CLI that rejected the flag form, so
        // one older than the 1.52 rework. That CLI has no `--bg` either, since
        // it came with the same rework. Carrying `--bg` here would make the
        // fallback fail on the only installs it exists for.
        let (_, legacy) = serve_arg_forms(5252);
        assert_eq!(legacy, ["serve", "https", "/", "http://127.0.0.1:5252"]);
        assert!(!legacy.iter().any(|a| a == "--bg"));
    }

    #[test]
    fn a_double_failure_keeps_the_legacy_reason_too() {
        // The pre-1.52 case, which is the only one the fallback exists for: the
        // current form fails as an unknown flag whatever else is wrong, so the
        // legacy attempt holds the one line worth reading. Reporting only the
        // current error would hand that user "unknown flag" for a daemon that
        // is not running.
        let both = both_serve_forms_failed(
            "tailscale serve failed: flag provided but not defined: -https",
            "tailscale serve failed: failed to connect to local tailscaled",
        );
        assert!(both.contains("failed to connect to local tailscaled"));
        assert!(both.contains("flag provided but not defined"));
        // Current first, and the retry labelled, so a modern CLI's failure does
        // not read as a syntax problem the user does not have.
        assert!(both.starts_with("tailscale serve failed: flag provided"));
        assert!(both.contains("older `serve https /` syntax"));
    }

    #[test]
    fn an_identical_double_failure_is_not_said_twice() {
        // Both forms hit the same wall (no tailnet, no permission), so there is
        // one reason, not two.
        let same = "tailscale serve failed: access denied";
        assert_eq!(both_serve_forms_failed(same, same), same);
    }

    #[test]
    fn neither_serve_form_can_reach_funnel_or_leave_loopback() {
        // The security invariant behind hardcoding this argv at all. `funnel`
        // is the adjacent subcommand that publishes to the open internet, and
        // the engine has no inbound API auth, so no attempt may name it. The
        // proxy target stays on loopback for the same reason: `serve` reaches
        // it from this machine, nothing else has to.
        let (current, legacy) = serve_arg_forms(5252);
        for form in [&current, &legacy] {
            assert_eq!(form.first().map(String::as_str), Some("serve"));
            assert!(
                !form.iter().any(|a| a.contains("funnel")),
                "{form:?} must never invoke funnel"
            );
            assert!(
                form.iter().any(|a| a.starts_with("http://127.0.0.1:")),
                "{form:?} must proxy to loopback"
            );
        }
    }

    #[test]
    fn the_approval_url_is_taken_verbatim_from_the_cli() {
        // The node id cannot be reconstructed, so the link is only ever the one
        // the CLI printed. Losing it is the whole reported bug.
        assert_eq!(
            tailnet_approval_url(SERVE_NOT_ENABLED).as_deref(),
            Some("https://login.tailscale.com/f/serve?node=nodeidEXAMPLE1234")
        );
        assert_eq!(tailnet_approval_url("No serve config\n"), None);
        assert_eq!(tailnet_approval_url(""), None);
    }

    #[test]
    fn only_a_tailscale_https_url_is_ever_offered() {
        // This is subprocess output on its way to the system browser, so the
        // scheme and the host are both checked before it is handed on.
        assert!(is_tailscale_https_url(
            "https://login.tailscale.com/f/serve?node=abc"
        ));
        assert!(is_tailscale_https_url("https://tailscale.com/kb/1242"));
        assert!(!is_tailscale_https_url(
            "http://login.tailscale.com/f/serve"
        ));
        assert!(!is_tailscale_https_url("https://evil.example/f/serve"));
        assert!(!is_tailscale_https_url("file:///etc/passwd"));
        // A lookalike host, and the userinfo trick that reads as one.
        assert!(!is_tailscale_https_url(
            "https://tailscale.com.evil.example/"
        ));
        assert!(!is_tailscale_https_url(
            "https://login.tailscale.com@evil.example/"
        ));
        // The real host behind userinfo still counts.
        assert!(is_tailscale_https_url(
            "https://user@login.tailscale.com/f/serve"
        ));
    }

    #[test]
    fn a_syntax_rejection_is_told_apart_from_every_other_failure() {
        // Only the first of these earns a retry under the older syntax. The
        // second is what a MODERN CLI says about the LEGACY form, so treating
        // it as a reason to retry is backwards. The third is a deadline, which
        // is not the CLI declining anything.
        assert!(is_flag_rejection(
            "tailscale serve failed: flag provided but not defined: -bg"
        ));
        assert!(!is_flag_rejection(
            "Error: the CLI for serve and funnel has changed. You can run the following \
             command instead:\n  - tailscale serve --bg http://127.0.0.1:5252"
        ));
        assert!(!is_flag_rejection(
            "tailscale serve did not finish within 20s and was stopped"
        ));
        assert!(!is_flag_rejection(
            "failed to connect to local tailscaled; is the tailscale daemon running?"
        ));
    }

    #[test]
    fn the_version_skew_warning_is_never_the_error() {
        // Printed on stderr by every command when a Homebrew CLI sits beside the
        // Mac app's daemon, so with streamed stderr it would lead every message.
        let cleaned = without_version_skew_warning(SERVE_NOT_ENABLED);
        assert!(!cleaned.contains("Warning: client version"), "{cleaned}");
        assert!(cleaned.starts_with("Serve is not enabled on your tailnet."));
        assert!(cleaned.contains("https://login.tailscale.com/f/serve?node="));
        // A message with no warning in it is left exactly as it was.
        assert_eq!(
            without_version_skew_warning("access denied"),
            "access denied"
        );
    }

    #[test]
    fn a_killed_attempt_still_reports_what_the_child_said() {
        // The reported bug in one test: the CLI printed the approval link and
        // then blocked, and reading the pipes only after exit meant the kill
        // took the link with it.
        //
        // The deadline has to outlast forking `/bin/sh`, not just the printf.
        // At 1500ms this flaked on a loaded host, killing the stub before it
        // had said anything. What is under test is the ORDERING, so the margin
        // is free to grow.
        let cancel = AtomicBool::new(false);
        let err = supervise_serve(
            sh("printf 'something worth reading\\n'; sleep 30"),
            &cancel,
            Duration::from_secs(4),
            Duration::from_secs(30),
            &mut no_phases(),
        )
        .expect_err("the deadline must fire");
        let ServeAttemptError::Failed(message) = err else {
            panic!("a deadline is not a flag rejection and not a cancel");
        };
        assert!(message.contains("did not finish within"), "{message}");
        assert!(message.contains("something worth reading"), "{message}");
    }

    #[test]
    fn the_tailnet_notice_becomes_a_phase_and_buys_the_child_more_time() {
        // The child is not stalled, it is waiting for a human. So the short
        // configure deadline must give way to the long approval one, and the
        // run must then finish by itself when the approval lands.
        //
        // Proved by OUTCOME rather than by a stopwatch, so host load cannot
        // decide it. The stub prints the notice, keeps going past the configure
        // deadline, then exits 0 the way the real CLI does once Serve is
        // enabled. Succeeding is only possible if the deadline moved.
        //
        // The notice still has to ARRIVE before the configure deadline, which
        // is a stopwatch after all. At 500ms a loaded host lost that race, so
        // both numbers grew and the gap between them held.
        let cancel = AtomicBool::new(false);
        let mut phases = Vec::new();
        let result = supervise_serve(
            sh(&format!(
                "printf '%s' '{SERVE_NOT_ENABLED}'; sleep 3; exit 0"
            )),
            &cancel,
            Duration::from_millis(1500),
            Duration::from_secs(30),
            &mut |p| phases.push(p),
        );
        assert!(
            result.is_ok(),
            "the run died before the approval landed, so the configure deadline was still in force"
        );
        assert_eq!(
            phases,
            [ServePhase::AwaitingTailnetApproval {
                url: "https://login.tailscale.com/f/serve?node=nodeidEXAMPLE1234".to_string(),
            }],
            "the notice must be announced exactly once"
        );
    }

    #[test]
    fn an_approval_that_never_comes_says_what_to_do_about_it() {
        // The other half: give up eventually, and hand back the link rather than
        // a bare deadline. This is the message the reported run should have shown.
        let cancel = AtomicBool::new(false);
        let err = supervise_serve(
            sh(&format!("printf '%s' '{SERVE_NOT_ENABLED}'; sleep 30")),
            &cancel,
            Duration::from_secs(5),
            Duration::from_millis(700),
            &mut no_phases(),
        )
        .expect_err("the approval deadline must fire");
        let ServeAttemptError::Failed(message) = err else {
            panic!("a deadline is not a flag rejection and not a cancel");
        };
        assert!(
            message.contains("Enable it at https://login.tailscale.com/f/serve?node="),
            "{message}"
        );
        // Not the bare stall wording: this run was waiting for the user, and
        // saying "did not finish" sends them looking for a fault instead.
        assert!(!message.contains("did not finish within"), "{message}");
    }

    #[test]
    fn a_cancel_stops_the_child_and_is_not_a_failure() {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let err = supervise_serve(
            sh("sleep 30"),
            &cancel,
            Duration::from_secs(30),
            Duration::from_secs(30),
            &mut no_phases(),
        )
        .expect_err("a cancel ends the attempt");
        assert!(matches!(err, ServeAttemptError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}, so the cancel was not observed",
            started.elapsed()
        );
    }

    #[test]
    fn a_rejected_flag_is_the_only_failure_that_asks_for_the_older_syntax() {
        let cancel = AtomicBool::new(false);
        let rejected = supervise_serve(
            sh("printf 'flag provided but not defined: -bg\\n' >&2; exit 1"),
            &cancel,
            Duration::from_secs(5),
            Duration::from_secs(5),
            &mut no_phases(),
        )
        .expect_err("exit 1 is a failure");
        assert!(matches!(rejected, ServeAttemptError::FlagRejected(_)));

        let other = supervise_serve(
            sh("printf 'failed to connect to local tailscaled\\n' >&2; exit 1"),
            &cancel,
            Duration::from_secs(5),
            Duration::from_secs(5),
            &mut no_phases(),
        )
        .expect_err("exit 1 is a failure");
        let ServeAttemptError::Failed(message) = other else {
            panic!("a daemon that is down is not a syntax problem");
        };
        assert!(message.contains("failed to connect to local tailscaled"));
    }

    #[test]
    fn output_still_in_flight_at_exit_is_waited_for_before_classifying() {
        // `try_wait` seeing a child terminate does not mean its output has been
        // collected, and the classification below reads that buffer: an empty one
        // reads as "not a flag rejection" and silently skips the legacy fallback
        // that exists for exactly the pre-1.52 CLI producing this line.
        //
        // Written as a child that exits while something else still holds the
        // pipe, because that is the DETERMINISTIC form of the hazard. The
        // scheduling-race form is the same bug and the same fix, but it cannot
        // be provoked reliably. The 100ms poll almost always hands the reader
        // its slack, so a test written that way guards nothing.
        let cancel = AtomicBool::new(false);
        let err = supervise_serve(
            sh("( sleep 0.2; printf 'flag provided but not defined: -bg\\n' >&2 ) & exit 1"),
            &cancel,
            Duration::from_secs(5),
            Duration::from_secs(5),
            &mut no_phases(),
        )
        .expect_err("exit 1 is a failure");
        assert!(
            matches!(err, ServeAttemptError::FlagRejected(_)),
            "classified before the output landed, so the fallback would be skipped"
        );
    }

    #[test]
    fn a_pipe_nothing_closes_does_not_hold_the_run_open() {
        // The other side of that wait: it is BOUNDED. A child that leaves a
        // grandchild holding the pipe would otherwise block the reader forever,
        // turning the collection step into a hang.
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let err = supervise_serve(
            sh("( sleep 30 ) & exit 1"),
            &cancel,
            Duration::from_secs(30),
            Duration::from_secs(30),
            &mut no_phases(),
        )
        .expect_err("exit 1 is a failure");
        assert!(matches!(err, ServeAttemptError::Failed(_)));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}, so the drain wait is not bounded",
            started.elapsed()
        );
    }

    #[test]
    fn a_clean_exit_is_a_success() {
        let cancel = AtomicBool::new(false);
        assert!(supervise_serve(
            sh("exit 0"),
            &cancel,
            Duration::from_secs(5),
            Duration::from_secs(5),
            &mut no_phases(),
        )
        .is_ok());
    }

    #[test]
    fn only_one_expose_run_holds_the_slot() {
        // Tauri used to serialise these on the main thread by accident. Off it,
        // nothing but this stops two children racing to write the same mapping.
        let runs = MobileAccessRuns::default();
        let first = runs.start_serve().expect("the slot starts free");
        assert!(runs.start_serve().is_none(), "a second run must be refused");
        drop(first);
        assert!(
            runs.start_serve().is_some(),
            "the slot must free itself, including on an error path"
        );
    }

    #[test]
    fn cancelling_sets_the_running_runs_flag() {
        let runs = MobileAccessRuns::default();
        // Nothing running: a cancel is a no-op, not a panic.
        runs.cancel_serve();
        let slot = runs.start_serve().expect("the slot starts free");
        assert!(!slot.cancel.load(Ordering::SeqCst));
        runs.cancel_serve();
        assert!(slot.cancel.load(Ordering::SeqCst));
    }

    #[test]
    fn only_one_sign_in_runs_at_a_time() {
        let runs = MobileAccessRuns::default();
        let first = runs.start_up().expect("the slot starts free");
        assert!(
            runs.start_up().is_none(),
            "a second sign-in must be refused"
        );
        drop(first);
        assert!(
            runs.start_up().is_some(),
            "the slot must free itself, including on an error path"
        );
    }

    #[test]
    fn the_https_wait_settles_on_success_cancel_and_deadline() {
        let cancel = AtomicBool::new(false);
        // Answers on the third look.
        let mut looks = 0;
        assert!(wait_until(
            || {
                looks += 1;
                looks >= 3
            },
            &cancel,
            Duration::from_secs(5)
        ));
        // Never answers: bounded, not forever.
        let started = Instant::now();
        assert!(!wait_until(|| false, &cancel, Duration::from_millis(300)));
        assert!(started.elapsed() < Duration::from_secs(5));
        // A cancel wins over a probe that would have succeeded.
        cancel.store(true, Ordering::SeqCst);
        assert!(!wait_until(|| true, &cancel, Duration::from_secs(5)));
    }

    #[test]
    fn the_serve_phase_tags_are_the_ones_the_frontend_handles() {
        // The TypeScript mirror in `utils/tauri.ts` is a discriminated union on
        // these exact strings. A rename here that is not made there renders as
        // a blank step rather than failing to compile.
        let tag = |p: &ServePhase| {
            serde_json::to_value(p).unwrap()["phase"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(tag(&ServePhase::Starting), "starting");
        assert_eq!(tag(&ServePhase::CheckingTailnet), "checking-tailnet");
        assert_eq!(tag(&ServePhase::Configuring), "configuring");
        assert_eq!(
            tag(&ServePhase::AwaitingTailnetApproval {
                url: "https://login.tailscale.com/f/serve".to_string()
            }),
            "awaiting-tailnet-approval"
        );
        assert_eq!(tag(&ServePhase::WaitingForHttps), "waiting-for-https");
        assert_eq!(
            tag(&ServePhase::Done {
                url: "https://x.ts.net".to_string()
            }),
            "done"
        );
        assert_eq!(
            tag(&ServePhase::Failed {
                message: "no".to_string()
            }),
            "failed"
        );
        assert_eq!(tag(&ServePhase::Cancelled), "cancelled");
        // The payload field names travel with the tag.
        let approval = serde_json::to_value(ServePhase::AwaitingTailnetApproval {
            url: "https://login.tailscale.com/f/serve".to_string(),
        })
        .unwrap();
        assert_eq!(
            approval["url"],
            "https://login.tailscale.com/f/serve".to_string()
        );
    }

    #[test]
    fn every_command_in_this_file_runs_off_the_main_thread() {
        // Tauri runs a SYNC command on the main thread, and every command here
        // blocks: `serve` for as long as setup takes, `up` for an interactive
        // browser login, `get_connect_info` for a few seconds of probes. As sync
        // commands they froze the window, which is what the spinning-wait cursor
        // on the Expose button was. Read out of this very file so the rule
        // cannot be a stale comment.
        let source = include_str!("mobile.rs");
        let mut checked = 0;
        for (index, _) in source.match_indices("#[tauri::command]") {
            // The attribute's own occurrence inside this test's needle would
            // recurse; only real attributes sit at the start of a line.
            if index > 0 && source.as_bytes()[index - 1] != b'\n' {
                continue;
            }
            let after = &source[index..];
            let signature = after.lines().nth(1).unwrap_or_default();
            assert!(
                signature.contains("async fn"),
                "`{}` is a synchronous #[tauri::command], so Tauri would run it on the main \
                 thread and freeze the window",
                signature.trim()
            );
            checked += 1;
        }
        assert!(
            checked >= 4,
            "parsed {checked} commands, so the scan is not seeing this file"
        );
    }
}
