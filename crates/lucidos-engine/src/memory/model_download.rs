//! Cache-warming downloader for the embedding model, with real byte progress.
//!
//! `fastembed` exposes no progress hook: `InitOptions::with_show_download_progress`
//! only drives an indicatif bar on stderr, which no UI can read. So the engine
//! fetches the model files ITSELF into fastembed's own cache layout and then
//! lets
//! [`FastEmbedProvider::with_model`](super::fastembed::FastEmbedProvider::with_model)
//! load them warm.
//!
//! Three properties make that safe, and all are load-bearing:
//!
//! * **Same layout.** Everything here mirrors fastembed's `pull_from_hf`
//!   verbatim: `HF_HOME` overrides `FASTEMBED_CACHE_DIR` overrides
//!   `.fastembed_cache`, `HF_ENDPOINT` overrides the hub, and the repo id is
//!   `ModelInfo::model_code`. Drift there means the model downloads TWICE. It is
//!   also why `hf-hub` is a direct dependency pinned to the version fastembed
//!   resolves (see this crate's `Cargo.toml`). It is also why the shared default
//!   ([`apply_default_cache_dir`]) is applied by SETTING the environment
//!   variable rather than by giving [`cache_dir`] another fallback.
//! * **Local first.** Every required file is probed in the cache before any API
//!   object is built, through the same `Cache::repo().get()` lookup
//!   `ApiRepo::get` performs, so a warm cache makes ZERO network requests and an
//!   offline machine still brings memory online.
//! * **Bounded.** Every connect and every read has a deadline ([`HubTimings`]).
//!   `hf-hub`'s own client has none and offers no knob, so a proxy that
//!   accepts the connection and then sends nothing blocked the load forever.
//!   The UI then claimed a retry that never came, and the blocked thread kept
//!   the blob lock, so no later attempt could take over.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fastembed::TextEmbedding;
use futures::StreamExt;
use hf_hub::api::Progress;
use hf_hub::{Cache, Repo};
use reqwest::header::{
    HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_RANGE, ETAG, LOCATION, RANGE, USER_AGENT,
};
use reqwest::StatusCode;
use tokio::io::AsyncWriteExt;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// fastembed's own default when `FASTEMBED_CACHE_DIR` is unset
/// (`fastembed::common::DEFAULT_CACHE_DIR`, relative to the process CWD).
const DEFAULT_CACHE_DIR: &str = ".fastembed_cache";

/// fastembed's default hub endpoint when `HF_ENDPOINT` is unset.
const DEFAULT_ENDPOINT: &str = "https://huggingface.co";

/// Directory under [`crate::paths::user_cache_root_from`] holding the shared
/// model cache. `scripts/e2e-embedder.sh` and `scripts/e2e-packaged.sh` seed
/// the same path, so an e2e run and the dev workspaces warm one copy.
const SHARED_CACHE_DIR_NAME: &str = "fastembed";

/// The tokenizer/config files `fastembed::common::load_tokenizer_hf_hub` reads
/// on top of the model's own `model_file` + `additional_files`. Listed here
/// because fastembed pulls them by literal name rather than through `ModelInfo`.
const TOKENIZER_FILES: &[&str] = &[
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

/// Minimum wall-clock gap between two emitted frames. The download calls
/// [`Progress::update`] once per read chunk, which is thousands of times a
/// second; every frame becomes an SSE broadcast, so it has to be throttled.
const FRAME_INTERVAL: Duration = Duration::from_millis(250);

/// How many times a held blob lock is tried before the pass reports
/// [`CacheOutcome::PeerDownloading`]. Matches `hf_hub::api::sync::lock_file`
/// (one try plus five retries), so both downloaders give up on a peer alike.
const LOCK_ATTEMPTS: u32 = 6;

/// Relative redirects followed while asking the hub for a file's metadata. The
/// hub uses one or two; the cap only stops a redirect loop.
const MAX_HUB_REDIRECTS: usize = 10;

/// Response headers the hub answers a resolve request with.
const HEADER_REPO_COMMIT: &str = "x-repo-commit";
const HEADER_LINKED_ETAG: &str = "x-linked-etag";

/// Suffix of a blob still being written and of its lock file. Both are
/// `hf-hub`'s, so a partial file either downloader left behind resumes, and the
/// two lock each other out.
const PARTIAL_EXTENSION: &str = "part";
const LOCK_EXTENSION: &str = "lock";

/// The deadlines on every hub request.
///
/// There is deliberately no deadline on a whole download: the ONNX file is
/// hundreds of MB, and a slow but live link must be allowed to finish. A
/// stalled link is caught by `read` instead, which bounds each silence.
#[derive(Clone, Copy, Debug)]
struct HubTimings {
    /// TCP plus TLS handshake, per connection.
    connect: Duration,
    /// Longest gap between two reads, headers included.
    read: Duration,
    /// Pause between two tries of a held blob lock.
    lock_retry: Duration,
}

const HUB_TIMINGS: HubTimings = HubTimings {
    connect: Duration::from_secs(30),
    read: Duration::from_secs(60),
    lock_retry: Duration::from_secs(1),
};

/// One throttled byte-progress reading, aggregated across every file in the
/// download. `total_bytes` is what is KNOWN so far: a file's size is learned
/// only when that file starts, so the total grows as the set is worked
/// through. [`should_emit`] is what keeps that from walking the fraction
/// backwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadFrame {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

/// How a cache-warming pass ended.
///
/// [`PeerDownloading`](Self::PeerDownloading) is why this is an enum rather than
/// the "did anything get fetched" boolean it started as: with one cache shared
/// by every workspace on the machine, "another engine is already fetching this"
/// is a normal state of the world, and the caller has to tell it apart from a
/// failure so it neither backs off as if the network were down nor tells the
/// user memory is degraded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheOutcome {
    /// Every required file was already local. Nothing was fetched and the
    /// network was never touched.
    AlreadyCached,
    /// Files were fetched and the cache is now complete.
    Downloaded,
    /// Another process holds `hf-hub`'s download lock on a file this pass still
    /// needs, so the cache is NOT complete yet. Wait briefly and look again: the
    /// files land as soon as the peer finishes.
    PeerDownloading,
}

/// Sink for [`ensure_model_cached`]'s progress. Implemented by the engine's
/// background loader, which turns each frame into a load-state update plus a
/// transient SSE broadcast.
pub trait ModelDownloadObserver: Send + Sync {
    fn progressed(&self, frame: DownloadFrame);
}

/// Whole-percent completion of a frame; `0` when nothing is known yet so an
/// unknown total can never divide by zero.
fn percent(frame: DownloadFrame) -> u64 {
    if frame.total_bytes == 0 {
        return 0;
    }
    frame.downloaded_bytes.saturating_mul(100) / frame.total_bytes
}

/// Whether `next` is worth emitting, given the last frame that WAS emitted and
/// how long ago. Pure so the whole throttle is unit-testable without a clock.
///
/// Three rules, in order:
///
/// 1. The terminal frame always goes out (unless it would repeat the last one),
///    so the UI always lands on a real 100%.
/// 2. A non-terminal frame must never claim completion. More files may follow,
///    and the moment one starts, the known total jumps and the fraction would
///    visibly fall back from 100%.
/// 3. Otherwise: the first frame always goes out (it is what tells the UI a
///    download is happening at all), and after that a frame needs BOTH
///    [`FRAME_INTERVAL`] to have passed AND a whole percentage point of
///    progress. The strict `>` is also what makes the sequence monotonic: when
///    a new file enlarges the total, the fraction dips, and those frames are
///    simply skipped until it climbs past where it already was.
fn should_emit(
    last: Option<DownloadFrame>,
    next: DownloadFrame,
    since_last: Duration,
    terminal: bool,
) -> bool {
    if terminal {
        return last != Some(next);
    }
    if next.total_bytes == 0 || next.downloaded_bytes >= next.total_bytes {
        return false;
    }
    let Some(last) = last else {
        return true;
    };
    since_last >= FRAME_INTERVAL && percent(next) > percent(last)
}

/// Running byte totals across the whole file set.
///
/// The [`Progress`] contract is per-file and repeatable: `init` may be called
/// again for one file (a resumed download re-inits and then re-advances with
/// `update(resume_offset)`), and `finish` lands once, on the attempt that
/// completed. So `init` RESETS the in-flight file rather than accumulating, and
/// only `finish` folds it into the completed total.
#[derive(Default)]
struct DownloadState {
    /// Summed declared size of every file that has finished.
    completed_bytes: u64,
    /// Declared size of the file currently in flight (`0` when none).
    current_size: u64,
    /// Bytes read so far within the file currently in flight.
    current_bytes: u64,
    last_emitted: Option<DownloadFrame>,
    last_at: Option<Instant>,
}

impl DownloadState {
    fn frame(&self) -> DownloadFrame {
        DownloadFrame {
            downloaded_bytes: self.completed_bytes + self.current_bytes,
            total_bytes: self.completed_bytes + self.current_size,
        }
    }

    /// Emit through `observer` if the throttle allows it, recording what went
    /// out so the next call can be judged against it.
    fn maybe_emit(&mut self, observer: &dyn ModelDownloadObserver, now: Instant, terminal: bool) {
        let next = self.frame();
        let since_last = self.last_at.map_or(Duration::ZERO, |at| now - at);
        if !should_emit(self.last_emitted, next, since_last, terminal) {
            return;
        }
        self.last_emitted = Some(next);
        self.last_at = Some(now);
        observer.progressed(next);
    }
}

/// Per-file progress callback, on `hf-hub`'s [`Progress`] trait. The fetch
/// takes it by value, so one of these is handed over per file while the
/// accumulated state stays behind in the caller. Single-threaded by
/// construction (the files are fetched one after another on one task), hence
/// `RefCell` rather than a lock.
struct ProgressHandle<'a> {
    state: &'a RefCell<DownloadState>,
    observer: &'a dyn ModelDownloadObserver,
}

impl Progress for ProgressHandle<'_> {
    fn init(&mut self, size: usize, _filename: &str) {
        let mut state = self.state.borrow_mut();
        state.current_size = size as u64;
        state.current_bytes = 0;
        state.maybe_emit(self.observer, Instant::now(), false);
    }

    fn update(&mut self, size: usize) {
        let mut state = self.state.borrow_mut();
        state.current_bytes += size as u64;
        state.maybe_emit(self.observer, Instant::now(), false);
    }

    fn finish(&mut self) {
        let mut state = self.state.borrow_mut();
        // The declared size is authoritative for the total, so a resumed or
        // retried file folds in exactly once and at its true weight.
        state.completed_bytes += state.current_size;
        state.current_size = 0;
        state.current_bytes = 0;
    }
}

/// Cache directory fastembed will look in, resolved exactly as `pull_from_hf`
/// does: `HF_HOME` wins over `FASTEMBED_CACHE_DIR`, which wins over the
/// CWD-relative default.
pub fn cache_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HF_HOME") {
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    std::env::var("FASTEMBED_CACHE_DIR")
        .ok()
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR))
}

/// The shared per-user cache location, given the two environment values that
/// decide it. Pure so both branches are testable without touching process env.
///
/// `None` when neither is usable (no `HOME` under a bare service manager), which
/// leaves the CWD-relative default in place rather than inventing a path.
fn shared_cache_dir_from(xdg_cache_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    crate::paths::user_cache_root_from(xdg_cache_home, home)
        .map(|root| root.join(SHARED_CACHE_DIR_NAME))
}

/// Point this process's model cache at the shared per-user directory, unless
/// something already chose one.
///
/// The model is hundreds of MB and byte-identical for every workspace on the
/// machine, so it is cached per USER, not per workspace: one copy serves every
/// engine (see the ADR on the shared embedding-model cache).
///
/// Two properties matter:
///
/// * **Inheritance wins.** An explicit `HF_HOME` or `FASTEMBED_CACHE_DIR` is
///   never overwritten, which is what keeps a packaged install under its
///   app-data directory, a headless service under its data directory, and a
///   user's own Hugging Face cache theirs.
/// * **It sets the variable.** Giving [`cache_dir`] another fallback would not
///   do: `fastembed` reads `FASTEMBED_CACHE_DIR` itself inside
///   `InitOptions::new`, so a default only this module knew about would leave
///   the two halves reading different directories and the model would download
///   twice.
///
/// Called once, early in `main`, and deliberately BEFORE
/// `environment_variables::apply_to_process_env`, so a value the user stored in
/// Settings still wins. Best effort throughout: it never fails boot.
///
/// `workspace` is the LAST resort, used only when there is no per-user cache
/// root to share (no `HOME`, no `XDG_CACHE_HOME`) or it cannot be created. It
/// resolves to the pre-ADR-0061 per-workspace location, which is deliberate on
/// two counts: it sits inside the workspace's gitignored `.lucidos/`, where
/// fastembed's own CWD-relative `.fastembed_cache` default would NOT (a
/// workspace ignores `.lucidos/`, so the default would leave a
/// multi-hundred-MB directory showing up as untracked in the user's own repo);
/// and [`legacy_cache`](super::legacy_cache) already reads "the active cache IS
/// the legacy path" as live data, so nothing tries to migrate or reclaim it.
pub fn apply_default_cache_dir(workspace: &Path) {
    for already_chosen in ["HF_HOME", "FASTEMBED_CACHE_DIR"] {
        if std::env::var(already_chosen)
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
        {
            return;
        }
    }
    let shared = shared_cache_dir_from(
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    );
    match &shared {
        Some(dir) => match std::fs::create_dir_all(dir) {
            Ok(()) => {
                std::env::set_var("FASTEMBED_CACHE_DIR", dir);
                log!(
                    @Memory,
                    "Embedding-model cache: {} (shared by every workspace on this machine)",
                    dir.display()
                );
                return;
            }
            Err(e) => log!(
                @Memory,
                "Could not create the shared embedding-model cache at {}: {}",
                dir.display(),
                e
            ),
        },
        None => log!(
            @Memory,
            "Neither HOME nor XDG_CACHE_HOME is set, so there is no per-user cache to share"
        ),
    }

    let own = super::legacy_cache::legacy_cache_path(workspace);
    if let Err(e) = std::fs::create_dir_all(&own) {
        log!(
            @Memory,
            "Could not create this workspace's embedding-model cache at {}: {}. Leaving it to \
             fastembed's own default, '{}' relative to the working directory",
            own.display(),
            e,
            DEFAULT_CACHE_DIR
        );
        return;
    }
    std::env::set_var("FASTEMBED_CACHE_DIR", &own);
    log!(
        @Memory,
        "Embedding-model cache: {} (this workspace only, since there is no shared location to use)",
        own.display()
    );
}

/// Hub endpoint, mirroring fastembed's `HF_ENDPOINT` override.
fn endpoint() -> String {
    std::env::var("HF_ENDPOINT")
        .ok()
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
}

/// Every file `TextEmbedding::try_new` will ask the repo for, in the order we
/// want them fetched: the ONNX model first because it is ~99% of the bytes, so
/// the progress bar spends its life on the one file that actually takes time.
fn required_files(model_file: &str, additional_files: &[String]) -> Vec<String> {
    let mut files = Vec::with_capacity(1 + additional_files.len() + TOKENIZER_FILES.len());
    files.push(model_file.to_string());
    files.extend(additional_files.iter().cloned());
    files.extend(TOKENIZER_FILES.iter().map(|f| f.to_string()));
    files
}

/// The hub access token `hf-hub` would send: `~/.cache/huggingface/token`,
/// written by `huggingface-cli login`. The default models are public, so this
/// only matters for a rate-limited or gated hub, and its absence is normal.
fn hub_token() -> Option<String> {
    let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
    Cache::new(PathBuf::from(home).join(".cache/huggingface/hub")).token()
}

/// One file as fetched by [`Hub::fetch_file`].
#[derive(Debug, PartialEq, Eq)]
enum FileFetch {
    /// The file is in the cache now.
    Cached,
    /// Another process holds this lock file on the blob.
    PeerHoldsLock(PathBuf),
}

/// What the hub says about one file before any of its bytes are read.
#[derive(Debug)]
struct FileMetadata {
    /// The commit the revision resolved to, which names the snapshot.
    commit: String,
    /// The content hash, which names the blob.
    etag: String,
    size: u64,
}

/// A response header as text, or an error naming which one is missing.
fn header<'a>(response: &'a reqwest::Response, name: &str) -> Result<&'a str, BoxError> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            format!(
                "{} answered without a usable '{name}' header",
                response.url()
            )
            .into()
        })
}

/// A hub-supplied name that becomes a path component under the cache. A hash
/// is all it ever is. Anything else (a `/`, a `..`, a dot that
/// `with_extension` would cut) is refused, so nothing lands outside the repo.
fn cache_component(kind: &str, value: &str) -> Result<String, BoxError> {
    let clean = value.trim().replace('"', "");
    if clean.is_empty() || !clean.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!("the hub sent an unusable {kind}: {value:?}").into());
    }
    Ok(clean)
}

/// The total after the slash in `Content-Range: bytes 0-0/<total>`.
fn content_range_total(response: &reqwest::Response) -> Result<u64, BoxError> {
    let range = header(response, CONTENT_RANGE.as_str())?;
    range
        .rsplit('/')
        .next()
        .and_then(|total| total.parse().ok())
        .ok_or_else(|| {
            format!(
                "{} sent an unreadable Content-Range: {range}",
                response.url()
            )
            .into()
        })
}

/// A transport error with its causes. reqwest's own text stops at the top
/// level ("error decoding response body"), which hides the timeout under it.
fn transport_error(e: reqwest::Error) -> BoxError {
    let mut message = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message.into()
}

/// Take the blob's lock, or `None` if another process keeps it through every
/// try. Released when the returned file is dropped, which is what lets a
/// failed or cancelled fetch hand the blob to the next attempt.
async fn lock_blob(lock_path: &Path, retry: Duration) -> Result<Option<std::fs::File>, BoxError> {
    let file = std::fs::File::create(lock_path)?;
    for attempt in 1..=LOCK_ATTEMPTS {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        if attempt < LOCK_ATTEMPTS {
            tokio::time::sleep(retry).await;
        }
    }
    Ok(None)
}

/// Point `snapshots/<commit>/<file>` at the blob with a relative symlink, as
/// `hf-hub` does, so the cache stays valid wherever it is moved.
fn link_snapshot(repo_dir: &Path, metadata: &FileMetadata, file: &str) -> Result<(), BoxError> {
    let pointer = repo_dir.join("snapshots").join(&metadata.commit).join(file);
    if let Some(parent) = pointer.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // One `..` for the commit directory, one for `snapshots`, and one for each
    // directory inside the file's own name.
    let mut target = PathBuf::new();
    for _ in 0..=Path::new(file).components().count() {
        target.push("..");
    }
    target.push("blobs");
    target.push(&metadata.etag);
    // A link left by an interrupted earlier pass may dangle, which `exists`
    // reads as absent, so it is replaced rather than trusted.
    if pointer.symlink_metadata().is_ok() {
        if pointer.exists() {
            return Ok(());
        }
        std::fs::remove_file(&pointer)?;
    }
    std::os::unix::fs::symlink(target, &pointer)?;
    Ok(())
}

/// The engine's client for the model hub, writing `hf-hub`'s cache layout.
struct Hub {
    endpoint: String,
    /// Follows no redirects, so the hub's own headers can be read.
    no_redirect: reqwest::Client,
    /// Follows redirects to the CDN the bytes are served from. reqwest drops
    /// the `Authorization` header when a redirect changes host.
    follow: reqwest::Client,
    lock_retry: Duration,
}

impl Hub {
    fn new(endpoint: String, timings: HubTimings) -> Result<Self, BoxError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("lucidos/", env!("CARGO_PKG_VERSION"))),
        );
        if let Some(token) = hub_token() {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        // Native TLS, as `hf-hub`'s client used: it trusts the system's CA
        // store, which is where a corporate TLS-inspecting proxy's root lives.
        let client = |redirects: reqwest::redirect::Policy| {
            reqwest::Client::builder()
                .use_native_tls()
                .default_headers(headers.clone())
                .connect_timeout(timings.connect)
                .read_timeout(timings.read)
                .redirect(redirects)
                .build()
        };
        Ok(Self {
            endpoint,
            no_redirect: client(reqwest::redirect::Policy::none())?,
            follow: client(reqwest::redirect::Policy::default())?,
            lock_retry: timings.lock_retry,
        })
    }

    /// Where a file of `repo` resolves on this hub.
    fn url(&self, repo: &Repo, file: &str) -> String {
        format!(
            "{}/{}/resolve/{}/{file}",
            self.endpoint,
            repo.url(),
            repo.url_revision()
        )
    }

    /// Ask for the first byte and read the answer's headers, exactly as
    /// `hf-hub` does. Redirects that stay on the hub's origin are followed; the
    /// first other answer carries the commit and the etag. When that answer is
    /// a redirect to the CDN, the size comes from following it.
    async fn metadata(&self, url: &str) -> Result<FileMetadata, BoxError> {
        let mut current = reqwest::Url::parse(url)?;
        let mut response = None;
        for _ in 0..=MAX_HUB_REDIRECTS {
            let answer = self
                .no_redirect
                .get(current.clone())
                .header(RANGE, "bytes=0-0")
                .send()
                .await
                .map_err(transport_error)?;
            // Judged on the joined URL, not the header's shape: a
            // protocol-relative `//other.host/...` parses as relative too, and
            // following it here would send the token to that host.
            let same_origin = answer
                .status()
                .is_redirection()
                .then(|| answer.headers().get(LOCATION))
                .flatten()
                .and_then(|l| l.to_str().ok())
                .and_then(|l| current.join(l).ok())
                .filter(|next| next.origin() == current.origin());
            match same_origin {
                Some(next) => current = next,
                None => {
                    response = Some(answer);
                    break;
                }
            }
        }
        let response = response.ok_or_else(|| format!("{url} redirected too many times"))?;
        let status = response.status();
        if !status.is_success() && !status.is_redirection() {
            return Err(format!("{url} answered HTTP {status}").into());
        }
        let etag = match response.headers().get(HEADER_LINKED_ETAG) {
            Some(_) => header(&response, HEADER_LINKED_ETAG)?,
            None => header(&response, ETAG.as_str())?,
        };
        let etag = cache_component("etag", etag)?;
        let commit = cache_component("commit", header(&response, HEADER_REPO_COMMIT)?)?;
        let size = if status.is_redirection() {
            let sized = self
                .follow
                .get(current)
                .header(RANGE, "bytes=0-0")
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(transport_error)?;
            content_range_total(&sized)?
        } else {
            content_range_total(&response)?
        };
        Ok(FileMetadata { commit, etag, size })
    }

    /// Stream the file into `partial`, resuming from whatever it already holds.
    async fn download(
        &self,
        url: &str,
        partial: &Path,
        size: u64,
        file: &str,
        mut progress: impl Progress,
    ) -> Result<(), BoxError> {
        let mut out = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(partial)
            .await?;
        let mut written = out.metadata().await?.len();
        if written > size {
            out.set_len(0).await?;
            written = 0;
        }
        progress.init(size as usize, file);
        if written < size {
            let response = self
                .follow
                .get(url)
                .header(RANGE, format!("bytes={written}-"))
                .send()
                .await
                .map_err(transport_error)?;
            match response.status() {
                StatusCode::PARTIAL_CONTENT => {}
                // The server ignored the range and is sending the whole file.
                StatusCode::OK => {
                    out.set_len(0).await?;
                    written = 0;
                }
                status => return Err(format!("{url} answered HTTP {status}").into()),
            }
            progress.update(written as usize);
            let mut body = response.bytes_stream();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(transport_error)?;
                out.write_all(&chunk).await?;
                written += chunk.len() as u64;
                progress.update(chunk.len());
            }
            out.flush().await?;
            if written != size {
                return Err(format!(
                    "the connection for {url} ended after {written} of {size} bytes"
                )
                .into());
            }
        } else {
            progress.update(written as usize);
        }
        progress.finish();
        Ok(())
    }

    /// Fetch one file of `repo` into the cache at `cache_root`, laid out as
    /// `hf-hub` lays it out: `blobs/<etag>`, a `snapshots/<commit>/<file>`
    /// link to it, and `refs/<revision>` naming the commit.
    async fn fetch_file(
        &self,
        cache_root: &Path,
        repo: &Repo,
        file: &str,
        progress: impl Progress,
    ) -> Result<FileFetch, BoxError> {
        let url = self.url(repo, file);
        let metadata = self.metadata(&url).await?;
        let repo_dir = cache_root.join(repo.folder_name());
        let blob = repo_dir.join("blobs").join(&metadata.etag);
        std::fs::create_dir_all(repo_dir.join("blobs"))?;

        let lock_path = blob.with_extension(LOCK_EXTENSION);
        let Some(lock) = lock_blob(&lock_path, self.lock_retry).await? else {
            return Ok(FileFetch::PeerHoldsLock(lock_path));
        };
        // Blobs are named by content, so one a peer finished between this
        // pass's cache probe and its lock is already the right bytes.
        if !blob.exists() {
            let partial = blob.with_extension(PARTIAL_EXTENSION);
            self.download(&url, &partial, metadata.size, file, progress)
                .await?;
            std::fs::rename(&partial, &blob)?;
        }
        drop(lock);

        link_snapshot(&repo_dir, &metadata, file)?;
        Cache::new(cache_root.to_path_buf())
            .repo(repo.clone())
            .create_ref(&metadata.commit)?;
        Ok(FileFetch::Cached)
    }
}

/// What one file's fetch means for the pass.
///
/// The split that matters is lock contention versus everything else. A fetch
/// holds an exclusive lock on the blob for the WHOLE download. A waiter gives
/// up after [`LOCK_ATTEMPTS`] one-second tries, which is nothing against a
/// multi-hundred-MB file. So on a shared cache a parallel cold start has
/// exactly one winner, and every other engine lands here within seconds.
/// Treating that as a fetch failure would back each loser off for minutes. It
/// would also tell its user memory was degraded, while the download proceeds
/// normally one process over.
///
/// Anything else keeps the treatment it had: wrapped so it stays fetch-class
/// (see the two notes below) and therefore retried with backoff.
///
/// Pure, and takes the result rather than performing it, so both branches are
/// testable without a hub.
fn classify_download(
    model_id: &str,
    file: &str,
    result: Result<FileFetch, BoxError>,
) -> Result<CacheOutcome, BoxError> {
    match result {
        Ok(FileFetch::Cached) => Ok(CacheOutcome::Downloaded),
        Ok(FileFetch::PeerHoldsLock(lock)) => {
            log!(
                @Memory,
                "Another process is already fetching '{}' into the shared model cache (lock: {}); \
                 waiting for it rather than fetching a second copy",
                file,
                lock.display()
            );
            Ok(CacheOutcome::PeerDownloading)
        }
        // Two things this wrapper has to keep doing, both inherited from the
        // path fastembed used to own:
        //
        // 1. "failed to retrieve" is one of the markers
        //    `is_model_fetch_failure` keys on, and it is the phrase fastembed
        //    wrapped its own fetch errors with. Anything that goes wrong while
        //    pulling bytes therefore stays fetch-class and keeps its
        //    backoff-and-retry. Corruption is still caught later, by
        //    `with_model`, and classified on its own text.
        // 2. `init_error_message` adds the actionable half (the cache dir, the
        //    CA bundle, pre-seeding), which is written for precisely this
        //    cold-cache case and would otherwise have been lost when the
        //    download moved out of `with_model`.
        Err(e) => Err(super::fastembed::init_error_message(
            model_id,
            format!("failed to retrieve embedding-model file '{file}': {e}"),
        )),
    }
}

/// Make sure every file the model needs is in fastembed's cache, reporting byte
/// progress as it goes. The [`CacheOutcome`] says whether anything was fetched
/// (so a warm boot can stay silent) and whether the cache is actually complete
/// (it is not, if a peer holds the lock).
///
/// Only the *download* happens here; the ONNX session is still built by
/// `FastEmbedProvider::with_model`, which then finds everything local. Every
/// request is bounded by [`HUB_TIMINGS`], so this always returns.
pub async fn ensure_model_cached(
    model_id: &str,
    observer: &dyn ModelDownloadObserver,
) -> Result<CacheOutcome, BoxError> {
    let (model, _dimensions) = super::fastembed::resolve_model(model_id)?;
    let info = TextEmbedding::get_model_info(&model)?;
    let files = required_files(&info.model_file, &info.additional_files);
    let dir = cache_dir();

    // Local-first probe, using the same lookup `ApiRepo::get` performs. Done
    // BEFORE the hub client is built, so a fully warm cache touches the network zero
    // times rather than paying a metadata request per file.
    let cache_repo = Cache::new(dir.clone()).model(info.model_code.clone());
    let missing: Vec<&String> = files
        .iter()
        .filter(|f| cache_repo.get(f).is_none())
        .collect();
    if missing.is_empty() {
        return Ok(CacheOutcome::AlreadyCached);
    }

    log!(
        @Memory,
        "Fetching {} embedding-model file(s) for '{}' into {}",
        missing.len(),
        model_id,
        dir.display()
    );

    let hub = Hub::new(endpoint(), HUB_TIMINGS)?;
    let repo = Repo::model(info.model_code.clone());

    let state = RefCell::new(DownloadState::default());
    for file in missing {
        let fetched = hub
            .fetch_file(
                &dir,
                &repo,
                file,
                ProgressHandle {
                    state: &state,
                    observer,
                },
            )
            .await;
        // The peer case leaves WITHOUT a terminal frame, deliberately: this pass
        // did not complete the set, and reporting 100% for a cache that is still
        // missing files would be a lie the next pass has to walk back.
        if classify_download(model_id, file, fetched)? == CacheOutcome::PeerDownloading {
            return Ok(CacheOutcome::PeerDownloading);
        }
    }
    state
        .borrow_mut()
        .maybe_emit(observer, Instant::now(), true);
    Ok(CacheOutcome::Downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Any known model id: these tests exercise the classification, never the
    /// model, so which one it is does not matter as long as it resolves.
    const DEFAULT_MODEL_FOR_TESTS: &str = super::super::fastembed::DEFAULT_MODEL;

    fn frame(downloaded: u64, total: u64) -> DownloadFrame {
        DownloadFrame {
            downloaded_bytes: downloaded,
            total_bytes: total,
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        frames: Mutex<Vec<DownloadFrame>>,
    }

    impl ModelDownloadObserver for RecordingObserver {
        fn progressed(&self, frame: DownloadFrame) {
            self.frames.lock().unwrap().push(frame);
        }
    }

    impl RecordingObserver {
        fn frames(&self) -> Vec<DownloadFrame> {
            self.frames.lock().unwrap().clone()
        }
    }

    /// The first frame is what tells the UI a download is happening at all (it
    /// is what auto-opens the status toast), so it must never be throttled away.
    #[test]
    fn first_frame_always_emits() {
        assert!(should_emit(None, frame(0, 1000), Duration::ZERO, false));
    }

    /// hf-hub calls `update` per read chunk. Without the interval gate every
    /// chunk would become an SSE broadcast.
    #[test]
    fn a_burst_within_the_interval_collapses() {
        let last = Some(frame(0, 1000));
        // Well past a whole percent, but far too soon.
        assert!(!should_emit(
            last,
            frame(500, 1000),
            Duration::from_millis(10),
            false
        ));
        assert!(should_emit(last, frame(500, 1000), FRAME_INTERVAL, false));
    }

    /// Time alone is not enough: a slow trickle that has not moved a whole
    /// percent has nothing new to say.
    #[test]
    fn sub_percent_progress_is_withheld_however_long_it_took() {
        let last = Some(frame(500, 100_000));
        assert!(!should_emit(
            last,
            frame(999, 100_000),
            Duration::from_secs(60),
            false
        ));
        assert!(should_emit(
            last,
            frame(1_500, 100_000),
            Duration::from_secs(60),
            false
        ));
    }

    /// The core monotonicity guard. `hf-hub` only reveals a file's size when it
    /// starts, so finishing the big ONNX file and then starting the tokenizer
    /// enlarges the known total and drops the fraction. Those frames are
    /// skipped until it climbs back past where it already was.
    ///
    /// With the real weights (the ONNX file is ~99% of the bytes) it never does
    /// climb back past: the bar holds at 99% while the trailing files land, and
    /// the terminal frame takes it to 100%. That is the intended reading, not a
    /// gap in the rule.
    #[test]
    fn a_growing_total_never_walks_the_fraction_backwards() {
        // 99% of the model file: the last thing that went out.
        let last = Some(frame(99, 100));
        for behind in [
            frame(99, 120),  // tokenizer starts, total grows: 82%
            frame(108, 120), // 90%
            frame(119, 120), // 99%, equal but not greater
        ] {
            assert!(
                !should_emit(last, behind, Duration::from_secs(1), false),
                "{behind:?} would drop the bar back from 99%"
            );
        }
        // Only the terminal frame reopens it, at a real 100%.
        assert!(should_emit(last, frame(120, 120), Duration::ZERO, true));
    }

    /// The guard withholds frames, it does not latch. A trailing file big
    /// enough for the fraction to genuinely climb past the last mark resumes
    /// reporting.
    #[test]
    fn reporting_resumes_once_the_fraction_passes_the_last_mark() {
        let last = Some(frame(50, 100));
        // A second file doubles the known total: 50/200 is 25%, well behind.
        assert!(!should_emit(
            last,
            frame(50, 200),
            Duration::from_secs(1),
            false
        ));
        // 130/200 is 65%, past the 50% already shown.
        assert!(should_emit(
            last,
            frame(130, 200),
            Duration::from_secs(1),
            false
        ));
    }

    /// A non-terminal frame must never read as finished: another file may be
    /// about to start and enlarge the total.
    #[test]
    fn completion_is_withheld_until_the_terminal_frame() {
        let last = Some(frame(50, 100));
        assert!(!should_emit(
            last,
            frame(100, 100),
            Duration::from_secs(1),
            false
        ));
        assert!(should_emit(
            last,
            frame(100, 100),
            Duration::from_secs(1),
            true
        ));
    }

    /// The terminal frame ignores both gates so the UI always lands on a real
    /// 100%, but it does not repeat a frame that already said exactly that.
    #[test]
    fn terminal_frame_bypasses_the_gates_but_never_duplicates() {
        assert!(should_emit(
            Some(frame(90, 100)),
            frame(100, 100),
            Duration::ZERO,
            true
        ));
        assert!(!should_emit(
            Some(frame(100, 100)),
            frame(100, 100),
            Duration::ZERO,
            true
        ));
    }

    /// An unknown total cannot divide by zero, and cannot be reported as
    /// complete either.
    #[test]
    fn an_unknown_total_is_never_complete() {
        assert_eq!(percent(frame(0, 0)), 0);
        assert!(!should_emit(None, frame(0, 0), Duration::ZERO, false));
    }

    /// The aggregation contract against `hf-hub`'s real callback order: `init`
    /// may repeat for one file (each `download_from` attempt re-inits), and only
    /// `finish` folds a file into the completed total.
    #[test]
    fn state_aggregates_across_files_and_survives_a_retry() {
        let observer = RecordingObserver::default();
        let state = RefCell::new(DownloadState::default());
        let mut handle = ProgressHandle {
            state: &state,
            observer: &observer,
        };

        handle.init(1000, "onnx/model.onnx");
        handle.update(400);
        assert_eq!(state.borrow().frame(), frame(400, 1000));

        // Retry: hf-hub re-inits the same file, then replays the resume offset.
        // The total must NOT double.
        handle.init(1000, "onnx/model.onnx");
        assert_eq!(state.borrow().frame(), frame(0, 1000));
        handle.update(400);
        handle.update(600);
        assert_eq!(state.borrow().frame(), frame(1000, 1000));

        handle.finish();
        assert_eq!(state.borrow().frame(), frame(1000, 1000));

        // Second file: the known total grows by exactly its size.
        handle.init(200, "tokenizer.json");
        assert_eq!(state.borrow().frame(), frame(1000, 1200));
        handle.update(200);
        handle.finish();
        assert_eq!(state.borrow().frame(), frame(1200, 1200));
    }

    /// End to end over the state machine: whatever the chunk pattern, the
    /// observer only ever sees a non-decreasing sequence that never claims
    /// completion before the terminal frame.
    #[test]
    fn emitted_frames_are_monotonic_and_finish_at_one_hundred_percent() {
        let observer = RecordingObserver::default();
        let state = RefCell::new(DownloadState::default());
        {
            let mut handle = ProgressHandle {
                state: &state,
                observer: &observer,
            };
            handle.init(1_000_000, "onnx/model.onnx");
            for _ in 0..100 {
                handle.update(10_000);
            }
            handle.finish();
            handle.init(1_000, "tokenizer.json");
            handle.update(1_000);
            handle.finish();
        }
        state
            .borrow_mut()
            .maybe_emit(&observer, Instant::now(), true);

        let frames = observer.frames();
        assert!(!frames.is_empty(), "at least the first frame must go out");
        let mut previous = 0u64;
        for f in &frames[..frames.len() - 1] {
            assert!(
                f.downloaded_bytes < f.total_bytes,
                "a non-terminal frame claimed completion: {f:?}"
            );
            assert!(
                percent(*f) >= previous,
                "fraction went backwards at {f:?} (was {previous}%)"
            );
            previous = percent(*f);
        }
        let last = frames.last().copied().expect("checked non-empty");
        assert_eq!(
            last,
            frame(1_001_000, 1_001_000),
            "the terminal frame must report the full set as complete"
        );
    }

    /// The file list drives what gets probed and fetched, so its ORDER is
    /// load-bearing: the dominant ONNX file goes first, and the tokenizer files
    /// fastembed pulls by literal name are all present.
    #[test]
    fn required_files_lead_with_the_model_and_carry_the_tokenizer_set() {
        let files = required_files("onnx/model.onnx", &["onnx/model.onnx_data".to_string()]);
        assert_eq!(files[0], "onnx/model.onnx");
        assert_eq!(files[1], "onnx/model.onnx_data");
        for expected in TOKENIZER_FILES {
            assert!(files.iter().any(|f| f == expected), "missing {expected}");
        }
    }

    /// The cache location must track fastembed's `pull_from_hf` resolution
    /// exactly, or the model is fetched twice: once by us, once by fastembed.
    /// Serialized with the endpoint test below since both mutate process env.
    #[test]
    fn cache_dir_mirrors_fastembeds_resolution_order() {
        let _guard = env_lock();
        let restore = EnvRestore::capture(&["HF_HOME", "FASTEMBED_CACHE_DIR"]);

        std::env::remove_var("HF_HOME");
        std::env::remove_var("FASTEMBED_CACHE_DIR");
        assert_eq!(cache_dir(), PathBuf::from(DEFAULT_CACHE_DIR));

        std::env::set_var("FASTEMBED_CACHE_DIR", "/tmp/fe-cache");
        assert_eq!(cache_dir(), PathBuf::from("/tmp/fe-cache"));

        // HF_HOME wins, matching pull_from_hf's `env::var("HF_HOME")...unwrap_or(default)`.
        std::env::set_var("HF_HOME", "/tmp/hf-home");
        assert_eq!(cache_dir(), PathBuf::from("/tmp/hf-home"));

        drop(restore);
    }

    /// The whole point of [`CacheOutcome::PeerDownloading`]: a lock held by
    /// another engine is a normal state of a shared cache, so it must NOT
    /// surface as an error. If it did, the loser of a parallel cold start would
    /// back off for minutes and tell its user memory was degraded, for a
    /// download that is proceeding fine one process over.
    #[test]
    fn a_lock_held_by_a_peer_is_an_outcome_and_not_a_failure() {
        let locked = classify_download(
            DEFAULT_MODEL_FOR_TESTS,
            "onnx/model.onnx",
            Ok(FileFetch::PeerHoldsLock(PathBuf::from(
                "/cache/blobs/abc.lock",
            ))),
        )
        .expect("a peer's lock is not an error");
        assert_eq!(locked, CacheOutcome::PeerDownloading);
    }

    /// Everything else keeps the treatment it had: fetch-class, so the loader
    /// retries with backoff rather than giving up.
    #[test]
    fn any_other_hub_failure_stays_a_fetch_class_error() {
        let err = classify_download(
            DEFAULT_MODEL_FOR_TESTS,
            "onnx/model.onnx",
            Err("operation timed out".into()),
        )
        .expect_err("a real fetch failure must stay an error");
        assert!(
            super::super::fastembed::is_model_fetch_failure(err.as_ref()),
            "must classify as fetch so the loader keeps retrying: {err}"
        );
        assert!(
            err.to_string().contains("onnx/model.onnx"),
            "the message must name the file: {err}"
        );
    }

    #[test]
    fn a_completed_file_reports_as_downloaded() {
        assert_eq!(
            classify_download(
                DEFAULT_MODEL_FOR_TESTS,
                "tokenizer.json",
                Ok(FileFetch::Cached)
            )
            .expect("a completed download is not an error"),
            CacheOutcome::Downloaded
        );
    }

    const FAKE_REPO: &str = "example-org/tiny-model";
    const FAKE_COMMIT: &str = "c0ffee";
    const FAKE_ETAG: &str = "abc123";
    const FAKE_BLOB_LEN: usize = 1000;

    fn fake_blob() -> Vec<u8> {
        (0..FAKE_BLOB_LEN).map(|i| (i % 251) as u8).collect()
    }

    /// How the fake hub answers a body request.
    #[derive(Clone, Copy)]
    enum FakeBody {
        Whole,
        /// Send this many bytes, then keep the connection open in silence: a
        /// proxy that blackholes the rest.
        StallAfter(usize),
    }

    /// A one-file hub on loopback, speaking just enough HTTP/1.1. The metadata
    /// probe (`Range: bytes=0-0`) gets the hub's headers, and any other request
    /// gets the blob from its range start, per `body`.
    async fn fake_hub(body: FakeBody) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve_fake_hub(stream, body));
            }
        });
        format!("http://{addr}")
    }

    async fn serve_fake_hub(stream: tokio::net::TcpStream, body: FakeBody) {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut stream = BufReader::new(stream);
        let blob = fake_blob();
        loop {
            let mut range = None;
            loop {
                let mut line = String::new();
                if stream.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let line = line.trim_end().to_ascii_lowercase();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("range: bytes=") {
                    range = Some(value.to_string());
                }
            }
            let out = stream.get_mut();
            if range.as_deref() == Some("0-0") {
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\n{HEADER_REPO_COMMIT}: {FAKE_COMMIT}\r\n\
                     etag: \"{FAKE_ETAG}\"\r\ncontent-range: bytes 0-0/{FAKE_BLOB_LEN}\r\n\
                     content-length: 1\r\n\r\n"
                );
                let _ = out.write_all(head.as_bytes()).await;
                let _ = out.write_all(&blob[..1]).await;
                continue;
            }
            let start: usize = range
                .as_deref()
                .and_then(|r| r.split('-').next())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let rest = &blob[start..];
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\ncontent-range: bytes {start}-{}/{FAKE_BLOB_LEN}\r\n\
                 content-length: {}\r\n\r\n",
                FAKE_BLOB_LEN - 1,
                rest.len()
            );
            let _ = out.write_all(head.as_bytes()).await;
            match body {
                FakeBody::Whole => {
                    let _ = out.write_all(rest).await;
                }
                FakeBody::StallAfter(sent) => {
                    let _ = out.write_all(&rest[..sent]).await;
                    let _ = out.flush().await;
                    std::future::pending::<()>().await;
                }
            }
        }
    }

    /// Fetch the fake repo's `file` into `cache`, with `read` as the silence
    /// deadline and a lock retry short enough for a test.
    async fn fetch_fake(
        cache: &Path,
        endpoint: String,
        read: Duration,
        file: &str,
        observer: &dyn ModelDownloadObserver,
    ) -> Result<FileFetch, BoxError> {
        let timings = HubTimings {
            connect: Duration::from_secs(5),
            read,
            lock_retry: Duration::from_millis(10),
        };
        let hub = Hub::new(endpoint, timings).expect("build hub client");
        let state = RefCell::new(DownloadState::default());
        hub.fetch_file(
            cache,
            &Repo::model(FAKE_REPO.to_string()),
            file,
            ProgressHandle {
                state: &state,
                observer,
            },
        )
        .await
    }

    fn fake_blobs_dir(cache: &Path) -> PathBuf {
        cache
            .join(Repo::model(FAKE_REPO.to_string()).folder_name())
            .join("blobs")
    }

    /// The regression the deadlines exist for: a proxy that starts the body and
    /// then goes silent. Without a read deadline this fetch never returns. The
    /// loader then never counts a failed attempt, and the UI sits on its last
    /// progress frame for the life of the process.
    ///
    /// The lock half matters as much. A fetch stuck holding the blob lock made
    /// every later attempt read it as a peer download, forever.
    #[tokio::test]
    async fn a_body_that_goes_silent_fails_fetch_class_and_frees_the_blob() {
        let cache = tempfile::tempdir().expect("tempdir");
        let endpoint = fake_hub(FakeBody::StallAfter(100)).await;
        let observer = RecordingObserver::default();

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            fetch_fake(
                cache.path(),
                endpoint,
                Duration::from_millis(300),
                "onnx/model.onnx",
                &observer,
            ),
        )
        .await
        .expect("the fetch hung on a silent connection instead of timing out");

        let err = classify_download(DEFAULT_MODEL_FOR_TESTS, "onnx/model.onnx", result)
            .expect_err("a stalled download must fail");
        assert!(
            super::super::fastembed::is_model_fetch_failure(err.as_ref()),
            "a stall must stay fetch-class so the loader retries: {err}"
        );
        assert!(
            !observer.frames().is_empty(),
            "the download reported progress before it stalled"
        );
        let lock = std::fs::File::create(
            fake_blobs_dir(cache.path()).join(format!("{FAKE_ETAG}.{LOCK_EXTENSION}")),
        )
        .expect("open lock file");
        lock.try_lock()
            .expect("a failed fetch must release the blob lock for the next attempt");
    }

    /// The layout invariant against `hf-hub`'s own reader, which is what
    /// fastembed loads through: the file must be found by `CacheRepo::get`,
    /// through a nested name, with the bytes the hub sent.
    #[tokio::test]
    async fn a_fetched_file_is_where_hf_hubs_cache_lookup_finds_it() {
        let cache = tempfile::tempdir().expect("tempdir");
        let endpoint = fake_hub(FakeBody::Whole).await;
        let observer = RecordingObserver::default();

        let fetched = fetch_fake(
            cache.path(),
            endpoint,
            Duration::from_secs(5),
            "onnx/model.onnx",
            &observer,
        )
        .await
        .expect("fetch from the fake hub");
        assert_eq!(fetched, FileFetch::Cached);

        let found = Cache::new(cache.path().to_path_buf())
            .model(FAKE_REPO.to_string())
            .get("onnx/model.onnx")
            .expect("hf-hub's cache lookup must find the fetched file");
        assert_eq!(std::fs::read(found).expect("read cached file"), fake_blob());
    }

    /// A partial blob left by an earlier, interrupted pass is continued from
    /// its end rather than fetched again.
    #[tokio::test]
    async fn a_partial_blob_resumes_where_it_stopped() {
        let cache = tempfile::tempdir().expect("tempdir");
        let blobs = fake_blobs_dir(cache.path());
        std::fs::create_dir_all(&blobs).expect("blobs dir");
        std::fs::write(
            blobs.join(format!("{FAKE_ETAG}.{PARTIAL_EXTENSION}")),
            &fake_blob()[..400],
        )
        .expect("seed partial blob");
        let endpoint = fake_hub(FakeBody::Whole).await;
        let observer = RecordingObserver::default();

        fetch_fake(
            cache.path(),
            endpoint,
            Duration::from_secs(5),
            "tokenizer.json",
            &observer,
        )
        .await
        .expect("resume from the fake hub");

        assert_eq!(
            std::fs::read(blobs.join(FAKE_ETAG)).expect("read blob"),
            fake_blob()
        );
    }

    /// A lock some other holder keeps is a peer's download, not a failure.
    #[tokio::test]
    async fn a_blob_locked_elsewhere_is_reported_as_a_peer() {
        let cache = tempfile::tempdir().expect("tempdir");
        let blobs = fake_blobs_dir(cache.path());
        std::fs::create_dir_all(&blobs).expect("blobs dir");
        let lock_path = blobs.join(format!("{FAKE_ETAG}.{LOCK_EXTENSION}"));
        let held = std::fs::File::create(&lock_path).expect("create lock file");
        held.try_lock().expect("take the lock first");
        let endpoint = fake_hub(FakeBody::Whole).await;
        let observer = RecordingObserver::default();

        let fetched = fetch_fake(
            cache.path(),
            endpoint,
            Duration::from_secs(5),
            "onnx/model.onnx",
            &observer,
        )
        .await
        .expect("a held lock is not an error");
        assert_eq!(fetched, FileFetch::PeerHoldsLock(lock_path));
    }

    /// The shared location: an explicit `XDG_CACHE_HOME` wins, else
    /// `$HOME/.cache`, and the sub-path is the one the e2e scripts already seed.
    #[test]
    fn shared_cache_dir_prefers_xdg_cache_home_over_home() {
        assert_eq!(
            shared_cache_dir_from(Some("/tmp/xdg"), Some("/tmp/home")),
            Some(PathBuf::from("/tmp/xdg/lucidos/fastembed"))
        );
        assert_eq!(
            shared_cache_dir_from(None, Some("/tmp/home")),
            Some(PathBuf::from("/tmp/home/.cache/lucidos/fastembed"))
        );
        // Empty reads as unset, matching how `cache_dir` treats its own vars.
        assert_eq!(
            shared_cache_dir_from(Some(""), Some("/tmp/home")),
            Some(PathBuf::from("/tmp/home/.cache/lucidos/fastembed"))
        );
    }

    /// With neither variable there is no per-user root to share, so the caller
    /// keeps the CWD-relative default instead of inventing a path.
    #[test]
    fn shared_cache_dir_is_unresolvable_without_a_root() {
        assert_eq!(shared_cache_dir_from(None, None), None);
        assert_eq!(shared_cache_dir_from(Some(""), Some("")), None);
    }

    /// The whole point of the default is that it yields to an explicit choice:
    /// this is what keeps a packaged install under app-data, a headless service
    /// under its data dir, and a user's own `HF_HOME` theirs.
    #[test]
    fn the_default_never_overwrites_an_explicit_choice() {
        let _guard = env_lock();
        let restore = EnvRestore::capture(&["HF_HOME", "FASTEMBED_CACHE_DIR", "XDG_CACHE_HOME"]);
        let workspace = tempfile::tempdir().expect("tempdir");

        std::env::set_var("XDG_CACHE_HOME", "/tmp/should-not-be-used");
        std::env::remove_var("HF_HOME");
        std::env::set_var("FASTEMBED_CACHE_DIR", "/tmp/chosen-by-the-service");
        apply_default_cache_dir(workspace.path());
        assert_eq!(cache_dir(), PathBuf::from("/tmp/chosen-by-the-service"));

        // `HF_HOME` alone is equally a choice, and outranks the variable this
        // would otherwise set, so setting one would only confuse a later reader.
        std::env::remove_var("FASTEMBED_CACHE_DIR");
        std::env::set_var("HF_HOME", "/tmp/the-users-own-hub-cache");
        apply_default_cache_dir(workspace.path());
        assert_eq!(std::env::var("FASTEMBED_CACHE_DIR").ok(), None);

        drop(restore);
    }

    /// With no per-user cache root at all, the last resort is the workspace's
    /// own gitignored `.lucidos/`, NOT fastembed's CWD-relative default: the
    /// engine's working directory IS the workspace, and a workspace ignores
    /// `.lucidos/` but not `.fastembed_cache/`, so the default would leave a
    /// multi-hundred-MB directory sitting untracked in the user's own repo.
    #[test]
    fn without_a_per_user_root_the_cache_lands_inside_the_workspace() {
        let _guard = env_lock();
        // HOME is captured too: this is the one test that unsets it, and every
        // later test (and the real loader) would follow it into the wrong home.
        let restore =
            EnvRestore::capture(&["HF_HOME", "FASTEMBED_CACHE_DIR", "XDG_CACHE_HOME", "HOME"]);
        let workspace = tempfile::tempdir().expect("tempdir");

        std::env::remove_var("HF_HOME");
        std::env::remove_var("FASTEMBED_CACHE_DIR");
        std::env::remove_var("XDG_CACHE_HOME");
        std::env::remove_var("HOME");

        apply_default_cache_dir(workspace.path());

        let expected = workspace.path().join(".lucidos/fastembed");
        assert_eq!(cache_dir(), expected);
        assert!(expected.is_dir());

        drop(restore);
    }

    /// With nothing chosen, the shared per-user directory is created and
    /// EXPORTED, which is what makes `fastembed`'s own `get_cache_dir()` agree
    /// with this module's [`cache_dir`]. A default only `cache_dir` knew about
    /// would split the two and download the model twice.
    #[test]
    fn the_default_exports_the_shared_dir_so_fastembed_agrees() {
        let _guard = env_lock();
        let restore = EnvRestore::capture(&["HF_HOME", "FASTEMBED_CACHE_DIR", "XDG_CACHE_HOME"]);

        let root = tempfile::tempdir().expect("tempdir");
        let workspace = tempfile::tempdir().expect("tempdir");
        std::env::remove_var("HF_HOME");
        std::env::remove_var("FASTEMBED_CACHE_DIR");
        std::env::set_var("XDG_CACHE_HOME", root.path());

        apply_default_cache_dir(workspace.path());

        let expected = root.path().join("lucidos/fastembed");
        assert_eq!(
            std::env::var("FASTEMBED_CACHE_DIR").ok().map(PathBuf::from),
            Some(expected.clone()),
            "the variable fastembed itself reads must carry the shared path"
        );
        assert_eq!(cache_dir(), expected);
        assert!(expected.is_dir(), "the shared cache directory must exist");

        drop(restore);
    }

    #[test]
    fn endpoint_mirrors_the_hf_endpoint_override() {
        let _guard = env_lock();
        let restore = EnvRestore::capture(&["HF_ENDPOINT"]);

        std::env::remove_var("HF_ENDPOINT");
        assert_eq!(endpoint(), DEFAULT_ENDPOINT);

        std::env::set_var("HF_ENDPOINT", "https://hub.example");
        assert_eq!(endpoint(), "https://hub.example");

        drop(restore);
    }

    /// Process env is global; the two env tests above would race each other
    /// under the default multi-threaded test harness.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// fastembed resolves the repo id as `EmbeddingModel::to_string()`, whose
    /// `Display` impl returns the model's `model_code`. This downloader uses
    /// `model_code` directly, so the two agree only as long as that holds. If
    /// fastembed ever changes `Display`, we would fill one repo directory while
    /// it reads from another, and every model would download twice.
    #[test]
    fn repo_id_matches_the_one_fastembed_resolves() {
        for id in [
            super::super::fastembed::MODEL_BGE_SMALL_EN,
            super::super::fastembed::MODEL_MULTILINGUAL_E5_SMALL,
        ] {
            let (model, _) = super::super::fastembed::resolve_model(id).expect("known model id");
            let info = TextEmbedding::get_model_info(&model).expect("known model info");
            assert_eq!(
                model.to_string(),
                info.model_code,
                "fastembed's repo id for '{id}' no longer matches ModelInfo::model_code"
            );
        }
    }

    /// The cache-layout invariant: what [`ensure_model_cached`] writes must be
    /// exactly what `FastEmbedProvider` then reads, or the model is fetched
    /// twice (once by us, once by fastembed) and a first run takes double the
    /// time and bandwidth.
    ///
    /// Measured rather than inspected: the cache tree's byte size must not grow
    /// while the provider is built. That assertion holds whether this test finds
    /// the cache cold (it performs the download, and the byte frames are checked
    /// too) or warm (another gated test's `shared_embedder()` got there first),
    /// so it needs no env mutation and cannot race the other real-embedder
    /// tests, which share this process and this cache.
    #[cfg(feature = "real-embedder-tests")]
    #[test]
    fn test_downloaded_model_loads_without_a_second_fetch() {
        use super::super::fastembed::{is_model_fetch_failure, model_id_from_env};
        use super::super::provider::EmbeddingProvider;
        use super::super::FastEmbedProvider;

        // The env tests in this module repoint the cache while they run, which
        // would send this download into their temporary directories.
        let _guard = env_lock();
        let model_id = model_id_from_env();
        let observer = RecordingObserver::default();
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let outcome = match runtime.block_on(ensure_model_cached(&model_id, &observer)) {
            Ok(outcome) => outcome,
            // Same resilience contract as `shared_embedder()`: a HuggingFace
            // outage skips, it never reds the suite. A non-fetch error is a
            // real bug and still panics.
            Err(e) if is_model_fetch_failure(e.as_ref()) => {
                eprintln!(
                    "[real-embedder-tests] SKIP: embedding-model files unavailable \
                     (huggingface.co fetch failed): {e}"
                );
                return;
            }
            Err(e) => panic!("ensure_model_cached failed with a non-fetch error: {e}"),
        };

        let frames = observer.frames();
        match outcome {
            CacheOutcome::Downloaded => {
                let last = *frames
                    .last()
                    .expect("a real download must report at least one frame");
                assert_eq!(
                    last.downloaded_bytes, last.total_bytes,
                    "the terminal frame must report the set as complete: {last:?}"
                );
                assert!(
                    last.total_bytes > 100_000_000,
                    "the ONNX model alone is hundreds of MB; got {} bytes",
                    last.total_bytes
                );
            }
            CacheOutcome::AlreadyCached => {
                assert!(
                    frames.is_empty(),
                    "a warm cache must report nothing (and must not touch the network): {frames:?}"
                );
            }
            // Only reachable when a real engine on this machine is fetching the
            // same model right now. Nothing below can be asserted (the cache is
            // still incomplete), and it is not a defect in either process.
            CacheOutcome::PeerDownloading => {
                eprintln!(
                    "[real-embedder-tests] SKIP: another process holds the model-download lock"
                );
                return;
            }
        }

        let dir = cache_dir();
        let before = super::super::legacy_cache::dir_bytes(&dir);
        let provider =
            FastEmbedProvider::with_model(&model_id).expect("the cached model must load");
        assert_eq!(provider.model_id(), model_id);
        let after = super::super::legacy_cache::dir_bytes(&dir);
        assert_eq!(
            before,
            after,
            "building the provider grew {} by {} bytes, so fastembed re-fetched files this \
             module had already cached: the two disagree about the cache layout",
            dir.display(),
            after.saturating_sub(before)
        );

        // ...and with everything present, a second pass is a pure no-op.
        let second = RecordingObserver::default();
        assert_eq!(
            runtime
                .block_on(ensure_model_cached(&model_id, &second))
                .expect("warm pass must not fail"),
            CacheOutcome::AlreadyCached,
            "a warm cache must report nothing fetched"
        );
        assert!(second.frames().is_empty());

        // Installing a REAL provider must flip the slot to ready in one step.
        // Asserted here rather than in its own gated test because this is the
        // one place a genuine provider already exists: an installed embedder
        // still reporting itself as loading would spin the UI forever.
        use super::super::embedder_slot::{EmbedderSlot, EmbeddingModelLoadState};
        let slot = EmbedderSlot::empty();
        slot.set_load_state(EmbeddingModelLoadState::Downloading {
            downloaded_bytes: 1,
            total_bytes: 2,
        });
        slot.install(provider);
        assert!(slot.is_ready());
        assert_eq!(slot.load_state(), EmbeddingModelLoadState::Ready);
    }

    /// Puts the captured variables back exactly as they were, so a test that
    /// sets `HF_HOME` cannot redirect a later test's (or the real loader's)
    /// cache.
    struct EnvRestore(Vec<(&'static str, Option<String>)>);

    impl EnvRestore {
        fn capture(keys: &[&'static str]) -> Self {
            Self(keys.iter().map(|k| (*k, std::env::var(k).ok())).collect())
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}
