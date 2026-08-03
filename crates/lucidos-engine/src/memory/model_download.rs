//! Cache-warming downloader for the embedding model, with real byte progress.
//!
//! `fastembed` exposes no progress hook: `InitOptions::with_show_download_progress`
//! only drives an indicatif bar on stderr, which no UI can read. The `hf-hub`
//! crate underneath it does (`ApiRepo::download_with_progress` plus the
//! [`Progress`] trait), so the engine fetches the model files ITSELF into
//! fastembed's own cache layout and then lets
//! [`FastEmbedProvider::with_model`](super::fastembed::FastEmbedProvider::with_model)
//! load them warm.
//!
//! Two properties make that safe, and both are load-bearing:
//!
//! * **Same layout.** Everything here mirrors fastembed's `pull_from_hf`
//!   verbatim: `HF_HOME` overrides `FASTEMBED_CACHE_DIR` overrides
//!   `.fastembed_cache`, `HF_ENDPOINT` overrides the hub, and the repo id is
//!   `ModelInfo::model_code`. Drift there means the model downloads TWICE. It is
//!   also why `hf-hub` is a direct dependency pinned to the version fastembed
//!   resolves (see this crate's `Cargo.toml`).
//! * **Local first.** Every required file is probed in the cache before any API
//!   object is built, through the same `Cache::repo().get()` lookup
//!   `ApiRepo::get` performs, so a warm cache makes ZERO network requests and an
//!   offline machine still brings memory online.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fastembed::TextEmbedding;
use hf_hub::api::sync::ApiBuilder;
use hf_hub::api::Progress;
use hf_hub::Cache;

/// fastembed's own default when `FASTEMBED_CACHE_DIR` is unset
/// (`fastembed::common::DEFAULT_CACHE_DIR`, relative to the process CWD).
const DEFAULT_CACHE_DIR: &str = ".fastembed_cache";

/// fastembed's default hub endpoint when `HF_ENDPOINT` is unset.
const DEFAULT_ENDPOINT: &str = "https://huggingface.co";

/// The tokenizer/config files `fastembed::common::load_tokenizer_hf_hub` reads
/// on top of the model's own `model_file` + `additional_files`. Listed here
/// because fastembed pulls them by literal name rather than through `ModelInfo`.
const TOKENIZER_FILES: &[&str] = &[
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

/// Minimum wall-clock gap between two emitted frames. `hf-hub` calls
/// [`Progress::update`] once per read chunk, which is thousands of times a
/// second; every frame becomes an SSE broadcast, so it has to be throttled.
const FRAME_INTERVAL: Duration = Duration::from_millis(250);

/// One throttled byte-progress reading, aggregated across every file in the
/// download. `total_bytes` is what is KNOWN so far: `hf-hub` reveals a file's
/// size only when that file starts, so the total grows as the set is worked
/// through. [`should_emit`] is what keeps that from walking the fraction
/// backwards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadFrame {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
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
/// `hf-hub`'s callback contract is per-file and repeatable: `init` is called
/// again for each `download_from` attempt (a retry re-inits and then re-advances
/// with `update(resume_offset)`), and `finish` lands once, on the attempt that
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

/// Per-file `hf-hub` callback. `download_with_progress` takes its `Progress` by
/// value, so one of these is handed over per file while the accumulated state
/// stays behind in the caller. Single-threaded by construction (the whole
/// download runs inside one blocking call), hence `RefCell` rather than a lock.
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
fn cache_dir() -> PathBuf {
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

/// Make sure every file the model needs is in fastembed's cache, reporting byte
/// progress as it goes. Returns whether anything was actually fetched, so a warm
/// boot can stay silent.
///
/// Blocking: call from `spawn_blocking`. Only the *download* happens here; the
/// ONNX session is still built by `FastEmbedProvider::with_model`, which then
/// finds everything local.
pub fn ensure_model_cached(
    model_id: &str,
    observer: &dyn ModelDownloadObserver,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let (model, _dimensions) = super::fastembed::resolve_model(model_id)?;
    let info = TextEmbedding::get_model_info(&model)?;
    let files = required_files(&info.model_file, &info.additional_files);
    let dir = cache_dir();

    // Local-first probe, using the same lookup `ApiRepo::get` performs. Done
    // BEFORE any `ApiBuilder`, so a fully warm cache touches the network zero
    // times rather than paying a metadata request per file.
    let cache_repo = Cache::new(dir.clone()).model(info.model_code.clone());
    let missing: Vec<&String> = files
        .iter()
        .filter(|f| cache_repo.get(f).is_none())
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }

    log!(
        @Memory,
        "Fetching {} embedding-model file(s) for '{}' into {}",
        missing.len(),
        model_id,
        dir.display()
    );

    let api = ApiBuilder::new()
        .with_cache_dir(dir)
        .with_endpoint(endpoint())
        // Our observer IS the progress reporting; hf-hub's own indicatif bar
        // would just scribble on the engine log.
        .with_progress(false)
        .build()?;
    let repo = api.model(info.model_code.clone());

    let state = RefCell::new(DownloadState::default());
    for file in missing {
        repo.download_with_progress(
            file,
            ProgressHandle {
                state: &state,
                observer,
            },
        )
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
        .map_err(|e| {
            super::fastembed::init_error_message(
                model_id,
                format!("failed to retrieve embedding-model file '{file}': {e}"),
            )
        })?;
    }
    state
        .borrow_mut()
        .maybe_emit(observer, Instant::now(), true);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

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

    /// Total bytes under `dir`, counting symlinks as themselves so hf-hub's
    /// `snapshots/<commit>/<file>` pointers are not summed on top of the
    /// `blobs/<etag>` files they target.
    #[cfg(feature = "real-embedder-tests")]
    fn cache_tree_bytes(dir: &std::path::Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| {
                let path = entry.path();
                match std::fs::symlink_metadata(&path) {
                    Ok(meta) if meta.is_dir() => cache_tree_bytes(&path),
                    Ok(meta) => meta.len(),
                    Err(_) => 0,
                }
            })
            .sum()
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

        let model_id = model_id_from_env();
        let observer = RecordingObserver::default();
        let downloaded = match ensure_model_cached(&model_id, &observer) {
            Ok(downloaded) => downloaded,
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
        if downloaded {
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
        } else {
            assert!(
                frames.is_empty(),
                "a warm cache must report nothing (and must not touch the network): {frames:?}"
            );
        }

        let dir = cache_dir();
        let before = cache_tree_bytes(&dir);
        let provider =
            FastEmbedProvider::with_model(&model_id).expect("the cached model must load");
        assert_eq!(provider.model_id(), model_id);
        let after = cache_tree_bytes(&dir);
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
        assert!(
            !ensure_model_cached(&model_id, &second).expect("warm pass must not fail"),
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
