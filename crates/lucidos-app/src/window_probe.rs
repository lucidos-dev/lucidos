//! A standalone reproduction for two macOS window-lifecycle faults.
//!
//! Behind the `window-probe` feature, so no shipped build carries it. It drives
//! AppKit and a bare `WKWebView` directly: no Tauri, no engine, no gateway and
//! no workspace, which is what makes it safe to run from a worktree.
//!
//! The two faults it answers, and what it found, are in ADR 0180:
//!
//!  1. **The page stops running while its window is on screen.** WebKit suspends
//!     the WebContent process when it reads the view as not visible, and a
//!     suspended process dispatches no input. The page reports a counter through
//!     `document.title`, which the UI process only hears from a live WebContent.
//!  2. **The traffic lights revert to AppKit's own position.** The probe places
//!     them once through the real [`crate::traffic_lights::inset_lights`], then
//!     never again, and reads the cluster back after every transition.
//!
//! It re-places nothing between steps, which is the reproduction: only a resize
//! re-applies in the client either. What it mirrors is written down in
//! [`Action`] and [`DRIVE`].

use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSEventMask, NSView, NSWindow, NSWindowOcclusionState, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

use crate::traffic_lights::{inset_lights, measure_cluster, LIGHTS_X_PX};

/// The page. It ticks once a second and publishes the count and WebKit's own
/// visibility verdict through the title. A suspended WebContent cannot keep that
/// channel fed, which is what makes it the detector.
const PAGE: &str = r#"<!doctype html><meta charset="utf-8">
<style>html,body{margin:0;height:100%;background:#0c52ad;color:#fff;
font:16px -apple-system,sans-serif}#o{padding:5rem 1.5rem}</style>
<body><div id="o">starting</div><script>
var n = 0;
function pub(){ document.title = n + '|' + document.visibilityState;
  document.getElementById('o').textContent =
    'tick ' + n + ' · ' + document.visibilityState; }
pub(); setInterval(function(){ n++; pub(); }, 1000);
</script>"#;

/// The bar the lights centre on when the caller names none. The desktop header
/// at the default UI scale, matching the shell's own fallback.
const DEFAULT_BAR_PX: f64 = 48.0;

/// How long a page may go silent, on an on-screen window, before the probe calls
/// it suspended. WebKit throttles a hidden page's timers to about one tick every
/// two seconds, so this has to clear that rather than trip on it.
const DEFAULT_STALL_SECS: f64 = 8.0;

/// How far the cluster's centre may sit from the bar's centre, in points.
const CENTRE_TOLERANCE_PT: f64 = 1.0;

/// How long to wait for the page's first tick before giving up on the run.
const FIRST_TICK_TIMEOUT: Duration = Duration::from_secs(20);

/// How long an off-screen step dwells. Long, deliberately: WebKit merely
/// THROTTLES a hidden page for the first several seconds, and only later drops
/// the assertion that suspends the process. A short dwell tests the throttle and
/// calls it a pass.
const OFF_SCREEN_SECS: u64 = 45;

/// How long an on-screen step dwells. It only has to outlast the stall window,
/// with room for the process to come back first.
const ON_SCREEN_SECS: u64 = 25;

/// How often a row prints when nothing is changing.
const QUIET_ROW_EVERY: Duration = Duration::from_secs(5);

/// What a step does to the window before its dwell. Each names a real client
/// path, so a reproduction here is a reproduction there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// The client's own launch order: `main` is declared hidden, placed during
    /// `setup`, and only then ordered in. This step is the ordering in, so the
    /// placement it is judged on was applied to a window nobody had shown.
    FirstShow,
    /// Nothing. The baseline every later step is read against.
    Settle,
    /// The close-to-tray park: order the window out, then drop the app to the
    /// menu-bar activation policy. `close_all_to_tray` then
    /// `enter_menu_bar_only_if_no_windows`.
    Park,
    /// The tray reopen: policy back to `Regular` FIRST, then order in, focus and
    /// activate. The order `front_window` takes.
    Reopen,
    Miniaturize,
    Deminiaturize,
    /// Cmd-H. AppKit orders every window out without the client asking, so
    /// nothing in the client records that the window left the screen.
    HideApp,
    UnhideApp,
    /// Re-apply the placement. The control step: it proves the arithmetic and
    /// the AppKit writes still work, so an earlier fault is a missing re-apply.
    RePlace,
}

/// One scripted step of the drive.
struct Step {
    name: &'static str,
    action: Action,
    dwell: Duration,
    /// Whether the window is on screen for this step. Only an on-screen step can
    /// fail: a window ordered out or miniaturized is *supposed* to suspend.
    on_screen: bool,
}

const fn on(name: &'static str, action: Action, secs: u64) -> Step {
    Step {
        name,
        action,
        dwell: Duration::from_secs(secs),
        on_screen: true,
    }
}

const fn off(name: &'static str, action: Action) -> Step {
    Step {
        name,
        action,
        dwell: Duration::from_secs(OFF_SCREEN_SECS),
        on_screen: false,
    }
}

/// The scripted sequence. Every off-screen step lasts long enough for WebKit to
/// reach a real suspend. Every on-screen step after one asks whether the page
/// came back.
const DRIVE: &[Step] = &[
    on("first-show", Action::FirstShow, ON_SCREEN_SECS),
    on("baseline", Action::Settle, ON_SCREEN_SECS),
    off("park", Action::Park),
    on("reopen", Action::Reopen, ON_SCREEN_SECS),
    off("miniaturize", Action::Miniaturize),
    on("deminiaturize", Action::Deminiaturize, ON_SCREEN_SECS),
    off("hide-app", Action::HideApp),
    on("unhide-app", Action::UnhideApp, ON_SCREEN_SECS),
    on("re-place", Action::RePlace, 5),
];

/// What the probe was asked to do.
struct Options {
    watch: bool,
    bar_px: f64,
    stall: Duration,
    /// Scales every dwell. `--quick` shrinks the run to a smoke test, which
    /// tests the throttle rather than the suspend and cannot clear the fault.
    scale: f64,
}

/// Read the command line, or say what is wrong with it.
fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        watch: false,
        bar_px: DEFAULT_BAR_PX,
        stall: Duration::from_secs_f64(DEFAULT_STALL_SECS),
        scale: 1.0,
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "drive" => options.watch = false,
            "watch" => options.watch = true,
            "--quick" => options.scale = 0.2,
            "--bar" => {
                let value = rest.next().ok_or("--bar needs a value in px")?;
                options.bar_px = value.parse().map_err(|_| format!("bad --bar {value:?}"))?;
            }
            "--stall" => {
                let value = rest.next().ok_or("--stall needs a value in seconds")?;
                let secs: f64 = value
                    .parse()
                    .map_err(|_| format!("bad --stall {value:?}"))?;
                options.stall = Duration::from_secs_f64(secs);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(options)
}

/// What the page last told us: its tick count, and WebKit's own verdict on
/// whether the page is visible.
#[derive(Debug, Clone, Default)]
struct PageState {
    ticks: u64,
    visibility: String,
}

/// The parsing half of [`read_page`], kept separate so it is testable with no
/// webview and no window server.
fn parse_page_title(title: &str) -> Option<PageState> {
    let (ticks, visibility) = title.split_once('|')?;
    Some(PageState {
        ticks: ticks.parse().ok()?,
        visibility: visibility.to_string(),
    })
}

/// Read the page's state out of the webview title. `None` before the first
/// publish, or for a title the page did not write.
fn read_page(web_view: &WKWebView) -> Option<PageState> {
    // SAFETY: reading the page title on the main thread. The binding is unsafe
    // only because a `WKWebView` subclass could override the getter.
    let title = unsafe { web_view.title() }?;
    parse_page_title(&title.to_string())
}

/// Run AppKit's event loop by hand until `until`.
///
/// A manual pump rather than `NSApp.run()`, so the probe keeps control between
/// steps. Events are dispatched, so the operator can drive the window in watch
/// mode.
fn pump(app: &NSApplication, until: Instant) {
    while Instant::now() < until {
        // SAFETY: the standard AppKit pump, on the main thread. The date is
        // autoreleased and lives for the length of this call.
        let event = unsafe {
            let deadline = NSDate::dateWithTimeIntervalSinceNow(0.05);
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&deadline),
                NSDefaultRunLoopMode,
                true,
            )
        };
        if let Some(event) = event {
            app.sendEvent(&event);
        }
    }
}

/// Build the probe window the way the client's is built.
///
/// Two details are copied on purpose. The style is `titleBarStyle: "Overlay"`,
/// so the webview owns the full height under a transparent band. And the
/// `WKWebView` is a SUBVIEW of a plain content view, which is wry's hierarchy,
/// rather than the content view itself.
fn build_window(mtm: MainThreadMarker) -> (Retained<NSWindow>, Retained<WKWebView>) {
    let size = NSSize::new(900.0, 620.0);
    let content = NSRect::new(NSPoint::new(240.0, 240.0), size);
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    // SAFETY: the designated initializer, called on the main thread with a
    // freshly allocated window.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            content,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setTitlebarAppearsTransparent(true);
    window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    window.setTitle(&NSString::from_str("Lucidos window probe"));
    // A close must not free the window under us: the probe keeps reading it.
    // SAFETY: main thread, and the window is ours alone.
    unsafe { window.setReleasedWhenClosed(false) };

    // SAFETY: a default configuration, on the main thread.
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
    // SAFETY: the designated initializer, on the main thread.
    let web_view = unsafe {
        WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration)
    };
    let resizes =
        NSAutoresizingMaskOptions::ViewHeightSizable | NSAutoresizingMaskOptions::ViewWidthSizable;
    web_view.setAutoresizingMask(resizes);
    // The parent wry puts between the window and the webview.
    let parent = NSView::initWithFrame(NSView::alloc(mtm), frame);
    parent.setAutoresizingMask(resizes);
    parent.addSubview(&web_view);
    window.setContentView(Some(&parent));
    window.makeFirstResponder(Some(&web_view));
    // SAFETY: a string literal page with no base URL, so nothing is resolved
    // against an origin.
    unsafe { web_view.loadHTMLString_baseURL(&NSString::from_str(PAGE), None) };
    (window, web_view)
}

/// The window's live AppKit state, as one comparable row.
#[derive(PartialEq, Eq, Clone, Copy)]
struct Shape {
    visible: bool,
    miniaturized: bool,
    occluded: bool,
}

fn shape_of(window: &NSWindow) -> Shape {
    Shape {
        visible: window.isVisible(),
        miniaturized: window.isMiniaturized(),
        occluded: !window
            .occlusionState()
            .contains(NSWindowOcclusionState::Visible),
    }
}

/// Prints a row when something changes, and otherwise on a slow tick. A row a
/// second over a three-minute run buries the transitions that matter.
struct Reporter {
    started: Instant,
    last: Option<(Shape, String)>,
    last_print: Instant,
}

impl Reporter {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            last: None,
            last_print: Instant::now() - QUIET_ROW_EVERY,
        }
    }

    fn row(
        &mut self,
        step: &str,
        window: &NSWindow,
        page: Option<&PageState>,
        silent_for: Duration,
        bar_px: f64,
    ) {
        let shape = shape_of(window);
        let visibility = page.map(|p| p.visibility.clone()).unwrap_or_default();
        let changed = self.last != Some((shape, visibility.clone()));
        if !changed && self.last_print.elapsed() < QUIET_ROW_EVERY {
            return;
        }
        self.last = Some((shape, visibility));
        self.last_print = Instant::now();

        let cluster = measure_cluster(window);
        let centre = cluster.map(|c| c.centre_from_top).unwrap_or(f64::NAN);
        let left = cluster.map(|c| c.left_x).unwrap_or(f64::NAN);
        println!(
            "t={:>4.0}s  {step:<14} visible={} mini={} occluded={}  ticks={:<5} silent={:>5.1}s  \
             page={:<8} centre={centre:>5.1} (want {:.1})  x={left:>4.1}",
            self.started.elapsed().as_secs_f64(),
            u8::from(shape.visible),
            u8::from(shape.miniaturized),
            u8::from(shape.occluded),
            page.map(|p| p.ticks.to_string())
                .unwrap_or_else(|| "-".to_string()),
            silent_for.as_secs_f64(),
            page.map(|p| p.visibility.as_str()).unwrap_or("-"),
            bar_px / 2.0,
        );
    }
}

/// Wait for the page to publish its first tick, so a run never blames the fault
/// for a page that simply had not loaded.
fn await_first_tick(app: &NSApplication, web_view: &WKWebView) -> Result<(), String> {
    let deadline = Instant::now() + FIRST_TICK_TIMEOUT;
    while Instant::now() < deadline {
        pump(app, Instant::now() + Duration::from_millis(200));
        if read_page(web_view).is_some() {
            return Ok(());
        }
    }
    Err(format!(
        "the page never published a tick within {FIRST_TICK_TIMEOUT:?}"
    ))
}

/// Apply one step's action to the window, in the order the client applies it.
fn apply(action: Action, app: &NSApplication, window: &NSWindow, bar_px: f64) {
    match action {
        Action::FirstShow => {
            window.makeKeyAndOrderFront(None);
            app.activate();
        }
        Action::Settle => {}
        Action::Park => {
            window.orderOut(None);
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
        Action::Reopen => {
            // Policy first. `Accessory` cannot front a window, which is why
            // `front_window` orders these calls this way.
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            window.deminiaturize(None);
            window.makeKeyAndOrderFront(None);
            app.activate();
        }
        Action::Miniaturize => window.miniaturize(None),
        Action::Deminiaturize => window.deminiaturize(None),
        Action::HideApp => app.hide(None),
        Action::UnhideApp => {
            app.unhide(None);
            app.activate();
        }
        Action::RePlace => inset_lights(window, LIGHTS_X_PX, bar_px),
    }
}

/// A fault the run observed, named by the step that produced it.
struct Fault {
    step: &'static str,
    what: String,
}

/// Tracks the page's liveness across a run.
struct Ticker {
    ticks: u64,
    last_change: Instant,
}

impl Ticker {
    fn new(web_view: &WKWebView) -> Self {
        Self {
            ticks: read_page(web_view).map(|p| p.ticks).unwrap_or(0),
            last_change: Instant::now(),
        }
    }

    /// Fold in what the page says now, and hand back that reading.
    fn observe(&mut self, web_view: &WKWebView) -> Option<PageState> {
        let page = read_page(web_view);
        if let Some(page) = &page {
            if page.ticks != self.ticks {
                self.ticks = page.ticks;
                self.last_change = Instant::now();
            }
        }
        page
    }
}

/// Judge one on-screen step, appending whatever it got wrong.
fn judge(
    entry: &Step,
    window: &NSWindow,
    ticker: &Ticker,
    options: &Options,
    out: &mut Vec<Fault>,
) {
    let silent_for = ticker.last_change.elapsed();
    if silent_for > options.stall {
        out.push(Fault {
            step: entry.name,
            what: format!(
                "the page stopped ticking for {:.0}s while the window was on screen",
                silent_for.as_secs_f64()
            ),
        });
    }
    let want = options.bar_px / 2.0;
    match measure_cluster(window) {
        Some(cluster) if (cluster.centre_from_top - want).abs() > CENTRE_TOLERANCE_PT => {
            out.push(Fault {
                step: entry.name,
                what: format!(
                    "the light cluster centres {:.1}pt down, wanted {want:.1}pt",
                    cluster.centre_from_top
                ),
            });
        }
        Some(_) => {}
        None => out.push(Fault {
            step: entry.name,
            what: "the window reported no standard buttons".to_string(),
        }),
    }
}

/// Run the scripted sequence. Returns every fault it saw.
fn drive(
    app: &NSApplication,
    window: &NSWindow,
    web_view: &WKWebView,
    options: &Options,
) -> Vec<Fault> {
    let mut faults = Vec::new();
    let mut ticker = Ticker::new(web_view);
    let mut reporter = Reporter::new();

    for entry in DRIVE {
        apply(entry.action, app, window, options.bar_px);
        let ends_at = Instant::now() + entry.dwell.mul_f64(options.scale);
        while Instant::now() < ends_at {
            pump(app, Instant::now() + Duration::from_millis(500));
            let page = ticker.observe(web_view);
            reporter.row(
                entry.name,
                window,
                page.as_ref(),
                ticker.last_change.elapsed(),
                options.bar_px,
            );
        }

        if entry.on_screen {
            judge(entry, window, &ticker, options, &mut faults);
        } else {
            // Not a fault either way, but the two answers mean different things.
            // Still ticking is WebKit not noticing the window left the screen.
            let verdict = if ticker.last_change.elapsed() < options.stall {
                "kept ticking (WebKit did not notice it left the screen)"
            } else {
                "went quiet, as an off-screen page should"
            };
            println!("note: {} {verdict}", entry.name);
        }
    }
    faults
}

/// Print rows until the operator stops the probe. Three transitions have no
/// public API and must be driven by hand: a Space switch, a move to a second
/// display, and a fullscreen round trip.
fn watch(app: &NSApplication, window: &NSWindow, web_view: &WKWebView, options: &Options) -> ! {
    println!(
        "[probe] watch mode. Drive the window yourself, Ctrl-C to stop. A row \
         reading visible=1 mini=0 with a rising silence is the suspend fault."
    );
    let mut ticker = Ticker::new(web_view);
    let mut reporter = Reporter::new();
    loop {
        pump(app, Instant::now() + Duration::from_millis(500));
        let page = ticker.observe(web_view);
        reporter.row(
            "watch",
            window,
            page.as_ref(),
            ticker.last_change.elapsed(),
            options.bar_px,
        );
    }
}

/// The probe's entry point. Returns the process exit code: zero when the run saw
/// neither fault.
pub fn run(args: &[String]) -> i32 {
    let options = match parse(args) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("[probe] {e}");
            eprintln!(
                "[probe] usage: window_lifecycle_probe [drive|watch] \
                 [--bar PX] [--stall S] [--quick]"
            );
            return 2;
        }
    };
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("[probe] must run on the main thread");
        return 2;
    };

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let (window, web_view) = build_window(mtm);
    app.finishLaunching();
    // Placed while the window is still hidden, which is when `setup` places
    // `main`. The first drive step is what orders it in. In watch mode there is
    // no drive, so show it here instead.
    inset_lights(&window, LIGHTS_X_PX, options.bar_px);
    println!(
        "[probe] placing against a {}px bar, so the cluster centre must sit {:.1}pt down",
        options.bar_px,
        options.bar_px / 2.0
    );
    match measure_cluster(&window) {
        Some(cluster) => println!(
            "[probe] placed while hidden: centre={:.1} x={:.1}",
            cluster.centre_from_top, cluster.left_x
        ),
        None => println!("[probe] placed while hidden: the window reports no buttons yet"),
    }
    if options.watch {
        window.makeKeyAndOrderFront(None);
        app.activate();
    }

    if let Err(e) = await_first_tick(&app, &web_view) {
        eprintln!("[probe] {e}");
        return 2;
    }
    if options.scale < 1.0 {
        println!("[probe] --quick: dwells are too short to reach a real suspend");
    }

    if options.watch {
        watch(&app, &window, &web_view, &options);
    }
    let faults = drive(&app, &window, &web_view, &options);
    if faults.is_empty() {
        println!("[probe] PASS: no fault reproduced");
        return 0;
    }
    for fault in &faults {
        println!("[probe] FAULT at {}: {}", fault.step, fault.what);
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_is_a_full_drive_at_the_default_bar() {
        let options = parse(&[]).expect("no arguments is a drive");
        assert!(!options.watch);
        assert_eq!(options.bar_px, DEFAULT_BAR_PX);
        assert_eq!(options.scale, 1.0);
    }

    #[test]
    fn watch_and_the_three_knobs_parse() {
        let args = ["watch", "--bar", "54", "--stall", "9", "--quick"].map(String::from);
        let options = parse(&args).expect("a valid command line");
        assert!(options.watch);
        assert_eq!(options.bar_px, 54.0);
        assert_eq!(options.stall, Duration::from_secs(9));
        assert!(options.scale < 1.0);
    }

    #[test]
    fn a_bad_command_line_says_what_is_wrong_rather_than_guessing() {
        for args in [
            vec!["--bar".to_string()],
            vec!["--bar".to_string(), "wide".to_string()],
            vec!["sideways".to_string()],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    /// An off-screen step has to outlast WebKit's throttle-then-suspend ramp, or
    /// the run tests the throttle and reports a pass.
    #[test]
    fn an_off_screen_step_dwells_long_enough_to_reach_a_suspend() {
        let stall = Duration::from_secs_f64(DEFAULT_STALL_SECS);
        for entry in DRIVE.iter().filter(|entry| !entry.on_screen) {
            assert!(
                entry.dwell >= stall * 4,
                "{} dwells {:?}",
                entry.name,
                entry.dwell
            );
        }
    }

    /// Every off-screen step is followed by an on-screen one. A park nobody
    /// reopens asks nothing.
    #[test]
    fn every_off_screen_step_is_followed_by_a_return_to_the_screen() {
        for pair in DRIVE.windows(2) {
            if !pair[0].on_screen {
                assert!(pair[1].on_screen, "{} is never undone", pair[0].name);
            }
        }
        assert!(DRIVE.last().is_some_and(|entry| entry.on_screen));
    }

    /// An on-screen step must outlast the stall window, or its silence could
    /// never be judged. The control step is exempt: it re-places and nothing
    /// else.
    #[test]
    fn every_on_screen_step_outlasts_the_stall_window() {
        let stall = Duration::from_secs_f64(DEFAULT_STALL_SECS);
        for entry in DRIVE.iter().filter(|entry| entry.on_screen) {
            assert!(
                entry.dwell > stall * 2 || entry.action == Action::RePlace,
                "{} dwells {:?}",
                entry.name,
                entry.dwell
            );
        }
    }

    /// The control step comes last, and it is the only one that re-places. A
    /// re-place anywhere earlier would hide the very revert being hunted.
    #[test]
    fn only_the_final_control_step_re_places() {
        let replacing: Vec<&str> = DRIVE
            .iter()
            .filter(|entry| entry.action == Action::RePlace)
            .map(|entry| entry.name)
            .collect();
        assert_eq!(replacing, vec!["re-place"]);
        assert_eq!(
            DRIVE.last().map(|entry| entry.action),
            Some(Action::RePlace)
        );
    }

    #[test]
    fn the_page_state_is_read_out_of_the_published_title() {
        assert_eq!(parse_page_title("17|visible").unwrap().ticks, 17);
        assert_eq!(parse_page_title("3|hidden").unwrap().visibility, "hidden");
        // A title the page did not write says nothing, rather than reading zero.
        assert!(parse_page_title("starting").is_none());
        assert!(parse_page_title("x|visible").is_none());
    }
}
