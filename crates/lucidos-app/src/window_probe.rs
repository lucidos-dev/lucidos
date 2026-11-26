use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSEventMask, NSScreen, NSView, NSWindow, NSWindowOcclusionState, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

use crate::traffic_lights::{
    inset_lights, measure_cluster, observe_relayouts, observe_resizes, retitle_and_place,
    stop_observing, titlebar_container, LIGHTS_X_PX,
};

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

const DEFAULT_BAR_PX: f64 = 48.0;

const DEFAULT_STALL_SECS: f64 = 8.0;

const CENTRE_TOLERANCE_PT: f64 = 1.0;

const FIRST_TICK_TIMEOUT: Duration = Duration::from_secs(20);

const OFF_SCREEN_SECS: u64 = 45;

const ON_SCREEN_SECS: u64 = 25;

const QUIET_ROW_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    FirstShow,
    Settle,
    Park,
    Reopen,
    Miniaturize,
    Deminiaturize,
    HideApp,
    UnhideApp,
    RePlace,
}

struct Step {
    name: &'static str,
    action: Action,
    dwell: Duration,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Drive,
    Watch,
    Screens,
}

struct Options {
    mode: Mode,
    bar_px: f64,
    stall: Duration,
    scale: f64,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        mode: Mode::Drive,
        bar_px: DEFAULT_BAR_PX,
        stall: Duration::from_secs_f64(DEFAULT_STALL_SECS),
        scale: 1.0,
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "drive" => options.mode = Mode::Drive,
            "watch" => options.mode = Mode::Watch,
            "screens" => options.mode = Mode::Screens,
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

#[derive(Debug, Clone, Default)]
struct PageState {
    ticks: u64,
    visibility: String,
}

fn parse_page_title(title: &str) -> Option<PageState> {
    let (ticks, visibility) = title.split_once('|')?;
    Some(PageState {
        ticks: ticks.parse().ok()?,
        visibility: visibility.to_string(),
    })
}

fn read_page(web_view: &WKWebView) -> Option<PageState> {
    // SAFETY: reading the page title on the main thread. The binding is unsafe
    // only because a `WKWebView` subclass could override the getter.
    let title = unsafe { web_view.title() }?;
    parse_page_title(&title.to_string())
}

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

struct Fault {
    step: &'static str,
    what: String,
}

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

const STOP_SETTLE: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Copy)]
struct Reading {
    scale: f64,
    centre: f64,
    left_x: f64,
    height: f64,
}

fn read_stop(window: &NSWindow) -> Reading {
    let cluster = measure_cluster(window);
    Reading {
        scale: window.backingScaleFactor(),
        centre: cluster.map(|c| c.centre_from_top).unwrap_or(f64::NAN),
        left_x: cluster.map(|c| c.left_x).unwrap_or(f64::NAN),
        height: window.frame().size.height,
    }
}

fn judge_stop(
    name: &'static str,
    stop: Reading,
    born: Reading,
    options: &Options,
    out: &mut Vec<Fault>,
) {
    let want = options.bar_px / 2.0;
    println!(
        "[probe] {name:<14} scale={:<4} height={:>6.1}  centre={:>5.1} (want {want:.1})  x={:>4.1}",
        stop.scale, stop.height, stop.centre, stop.left_x
    );
    if (stop.height - born.height).abs() > 0.5 {
        out.push(Fault {
            step: name,
            what: format!(
                "the window went from {:.1}pt tall to {:.1}pt, so this stop measures a resize",
                born.height, stop.height
            ),
        });
        return;
    }
    let off = (stop.centre - want).abs();
    if !off.is_finite() || off > CENTRE_TOLERANCE_PT {
        out.push(Fault {
            step: name,
            what: format!(
                "the light cluster centres {:.1}pt down, wanted {want:.1}pt",
                stop.centre
            ),
        });
    }
}

fn other_screen(window: &NSWindow, mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    let here = window.screen()?.frame().origin;
    let screens = NSScreen::screens(mtm);
    (0..screens.count())
        .map(|index| screens.objectAtIndex(index))
        .find(|screen| {
            let there = screen.frame().origin;
            there.x != here.x || there.y != here.y
        })
}

fn landing_origin(screen: &NSScreen, size: NSSize) -> NSPoint {
    let visible = screen.visibleFrame();
    NSPoint::new(
        visible.origin.x + (visible.size.width - size.width).max(0.0) / 2.0,
        visible.origin.y + (visible.size.height - size.height).max(0.0) / 2.0,
    )
}

fn walk_screens(
    app: &NSApplication,
    window: &NSWindow,
    options: &Options,
    mtm: MainThreadMarker,
) -> Vec<Fault> {
    let mut faults = Vec::new();
    let born = read_stop(window);
    judge_stop("placed-hidden", born, born, options, &mut faults);

    let home = window.frame().origin;
    window.setFrameOrigin(NSPoint::new(home.x + 80.0, home.y - 40.0));
    pump(app, Instant::now() + STOP_SETTLE);
    judge_stop(
        "moved-hidden",
        read_stop(window),
        born,
        options,
        &mut faults,
    );

    window.makeKeyAndOrderFront(None);
    app.activate();
    pump(app, Instant::now() + STOP_SETTLE);
    judge_stop("first-show", read_stop(window), born, options, &mut faults);

    window.setFrameOrigin(NSPoint::new(home.x + 20.0, home.y - 20.0));
    pump(app, Instant::now() + STOP_SETTLE);
    judge_stop(
        "same-display",
        read_stop(window),
        born,
        options,
        &mut faults,
    );

    window.setContentSize(window.contentRectForFrameRect(window.frame()).size);
    pump(app, Instant::now() + STOP_SETTLE);
    judge_stop(
        "restated-size",
        read_stop(window),
        born,
        options,
        &mut faults,
    );

    if let Some(other) = other_screen(window, mtm) {
        window.setFrameOrigin(landing_origin(&other, window.frame().size));
        pump(app, Instant::now() + STOP_SETTLE);
        judge_stop(
            "other-display",
            read_stop(window),
            born,
            options,
            &mut faults,
        );

        window.setFrameOrigin(home);
        pump(app, Instant::now() + STOP_SETTLE);
        judge_stop("back-home", read_stop(window), born, options, &mut faults);
    } else {
        println!("[probe] one display only: the cross-display stops need a second one attached");
    }

    let mut frame = window.frame();
    frame.size.height -= 60.0;
    frame.size.width -= 60.0;
    window.setFrame_display(frame, true);
    pump(app, Instant::now() + STOP_SETTLE);
    let after = read_stop(window);
    let mut control = Vec::new();
    judge_stop("resize-control", after, after, options, &mut control);
    if control.is_empty() {
        faults.push(Fault {
            step: "resize-control",
            what: "a resize left the placement alone, so this walk cannot see a revert at all"
                .to_string(),
        });
    }
    faults.extend(judge_the_re_apply(app, window, options));
    faults.extend(judge_the_retitle(app, window, options));
    faults.extend(judge_the_appearance(app, window, options));
    faults
}

fn flip_appearance(app: &NSApplication, dark: bool) {
    use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua};
    // SAFETY: AppKit's own appearance name constants, read on the main thread.
    let name = unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    app.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
}

fn judge_the_appearance(app: &NSApplication, window: &NSWindow, options: &Options) -> Vec<Fault> {
    let mut faults = Vec::new();
    let bar = options.bar_px;
    let to_dark = || {
        flip_appearance(app, false);
        pump(app, Instant::now() + STOP_SETTLE);
        inset_lights(window, LIGHTS_X_PX, bar);
        flip_appearance(app, true);
        pump(app, Instant::now() + STOP_SETTLE);
        read_stop(window)
    };

    let bare = to_dark();
    let mut control = Vec::new();
    judge_stop("appearance-bare", bare, bare, options, &mut control);
    if control.is_empty() {
        faults.push(Fault {
            step: "appearance-bare",
            what: "an appearance change left the placement alone, so this stop sees no revert"
                .to_string(),
        });
    }

    let Some(container) = window
        .standardWindowButton(objc2_app_kit::NSWindowButton::CloseButton)
        .and_then(|close| titlebar_container(&close))
    else {
        faults.push(Fault {
            step: "appearance",
            what: "the window has no titlebar container to observe".to_string(),
        });
        return faults;
    };
    let relayouts = std::rc::Rc::new(std::cell::Cell::new(0_u32));
    let counted = relayouts.clone();
    let observers = [
        observe_resizes(window, move |window| inset_lights(window, LIGHTS_X_PX, bar)),
        observe_relayouts(window, &container, move |window| {
            counted.set(counted.get() + 1);
            inset_lights(window, LIGHTS_X_PX, bar)
        }),
    ];

    let client = to_dark();
    judge_stop("appearance", client, client, options, &mut faults);

    retitle_and_place(window, "probe: retitle under both observers", bar);
    pump(app, Instant::now() + STOP_SETTLE);
    let retitled = read_stop(window);
    judge_stop("both-retitle", retitled, retitled, options, &mut faults);

    let mut frame = window.frame();
    frame.size.height -= 40.0;
    window.setFrame_display(frame, true);
    pump(app, Instant::now() + STOP_SETTLE);
    let resized = read_stop(window);
    judge_stop("both-resize", resized, resized, options, &mut faults);

    faults.extend(judge_full_screen(app, window, options, &relayouts));
    for observer in &observers {
        stop_observing(observer);
    }
    app.setAppearance(None);
    faults
}

const FULL_SCREEN_SETTLE: Duration = Duration::from_secs(3);

fn judge_full_screen(
    app: &NSApplication,
    window: &NSWindow,
    options: &Options,
    relayouts: &std::cell::Cell<u32>,
) -> Vec<Fault> {
    let mut faults = Vec::new();
    window.toggleFullScreen(None);
    pump(app, Instant::now() + FULL_SCREEN_SETTLE);
    window.toggleFullScreen(None);
    pump(app, Instant::now() + FULL_SCREEN_SETTLE);

    let settled = relayouts.get();
    pump(app, Instant::now() + STOP_SETTLE);
    if relayouts.get() != settled {
        faults.push(Fault {
            step: "full-screen",
            what: format!(
                "the relayout observer fired {} more times on an idle window",
                relayouts.get() - settled
            ),
        });
    }
    let back = read_stop(window);
    judge_stop("full-screen", back, back, options, &mut faults);
    faults
}

fn judge_the_retitle(app: &NSApplication, window: &NSWindow, options: &Options) -> Vec<Fault> {
    let mut faults = Vec::new();
    inset_lights(window, LIGHTS_X_PX, options.bar_px);

    window.setTitle(&NSString::from_str("probe: bare retitle"));
    pump(app, Instant::now() + STOP_SETTLE);
    let bare = read_stop(window);
    let mut control = Vec::new();
    judge_stop("retitle-bare", bare, bare, options, &mut control);
    if control.is_empty() {
        faults.push(Fault {
            step: "retitle-bare",
            what: "a new title left the placement alone, so the client's retitle re-place is dead"
                .to_string(),
        });
    }

    retitle_and_place(window, "probe: client retitle", options.bar_px);
    pump(app, Instant::now() + STOP_SETTLE);
    let client = read_stop(window);
    judge_stop("retitle", client, client, options, &mut faults);
    faults
}

fn judge_the_re_apply(app: &NSApplication, window: &NSWindow, options: &Options) -> Vec<Fault> {
    let mut faults = Vec::new();
    let bar = options.bar_px;
    let observer = observe_resizes(window, move |window| inset_lights(window, LIGHTS_X_PX, bar));

    let mut frame = window.frame();
    frame.size.height -= 40.0;
    window.setFrame_display(frame, true);
    pump(app, Instant::now() + STOP_SETTLE);
    let synchronous = read_stop(window);
    judge_stop(
        "re-apply-sync",
        synchronous,
        synchronous,
        options,
        &mut faults,
    );

    inset_lights(window, LIGHTS_X_PX, bar);
    pump(app, Instant::now() + STOP_SETTLE);
    let queued = read_stop(window);
    judge_stop("re-apply-late", queued, queued, options, &mut faults);

    stop_observing(&observer);
    faults
}

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

pub fn run(args: &[String]) -> i32 {
    let options = match parse(args) {
        Ok(options) => options,
        Err(e) => {
            eprintln!("[probe] {e}");
            eprintln!(
                "[probe] usage: window_lifecycle_probe [drive|watch|screens] \
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
    match measure_cluster(&window) {
        Some(cluster) => println!(
            "[probe] AppKit's own: centre={:.1} x={:.1}",
            cluster.centre_from_top, cluster.left_x
        ),
        None => println!("[probe] AppKit's own: the window reports no buttons yet"),
    }
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
    if options.mode == Mode::Screens {
        return report(walk_screens(&app, &window, &options, mtm));
    }
    if options.mode == Mode::Watch {
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

    if options.mode == Mode::Watch {
        watch(&app, &window, &web_view, &options);
    }
    report(drive(&app, &window, &web_view, &options))
}

fn report(faults: Vec<Fault>) -> i32 {
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
        assert_eq!(options.mode, Mode::Drive);
        assert_eq!(options.bar_px, DEFAULT_BAR_PX);
        assert_eq!(options.scale, 1.0);
    }

    #[test]
    fn the_display_walk_has_its_own_mode() {
        let options = parse(&["screens".to_string()]).expect("screens is a mode");
        assert_eq!(options.mode, Mode::Screens);
    }

    #[test]
    fn watch_and_the_three_knobs_parse() {
        let args = ["watch", "--bar", "54", "--stall", "9", "--quick"].map(String::from);
        let options = parse(&args).expect("a valid command line");
        assert_eq!(options.mode, Mode::Watch);
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

    #[test]
    fn every_off_screen_step_is_followed_by_a_return_to_the_screen() {
        for pair in DRIVE.windows(2) {
            if !pair[0].on_screen {
                assert!(pair[1].on_screen, "{} is never undone", pair[0].name);
            }
        }
        assert!(DRIVE.last().is_some_and(|entry| entry.on_screen));
    }

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
        assert!(parse_page_title("starting").is_none());
        assert!(parse_page_title("x|visible").is_none());
    }
}
