//! The driver against a real event log, a real repository and a fake check
//! script. Each test feeds the driver the messages the engine would, and
//! reads back the events and the snapshots it published.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use uuid::Uuid;

use super::driver::{Driver, DriverHooks, DriverSetup, MainCheckMsg, ENGINE_RESTARTED};
use super::fix_thread::{fix_thread_implementer, FixThreadOpening};
use super::runner::{spawn_runner, MainCheckCommand, ReportedVerdict, RunTarget, RunnerSlot};
use super::snapshot::{MainCheckSnapshot, MainCheckState};
use super::{forward_landings, landing_of, CloseRefusal, LANDING_EVENT_TYPES};
use crate::engine::event_bus::{BusEvent, EventBus, LagReplay, ReplayScope};
use crate::engine::git_ops::git_cmd;
use crate::engine::thread_events::{
    EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::test_support::{
    flood_past_bus_capacity, setup_test_db, start_cc_session, teardown_test_db,
};

const RUN_URL: &str = "https://example.invalid/actions/runs/7";
const RUN_BRANCH: &str = "e2e/20261019-120000-ab12";

/// A check script that names its run, records its sha, and reports `verdict`.
fn reporting_script(verdict: &str, summary: &str) -> String {
    reporting_script_with(verdict, summary, "")
}

/// [`reporting_script`] with `extra`, more JSON fields, after the tail.
fn reporting_script_with(verdict: &str, summary: &str, extra: &str) -> String {
    format!(
        "#!/bin/bash\necho \"$1\" >>\"$(dirname \"$0\")/shas\"\n\
         echo \"main-check run {RUN_URL} {RUN_BRANCH}\"\n\
         echo 'main-check verdict {{\"verdict\": \"{verdict}\", \"run_url\": \"{RUN_URL}\", \
         \"summary\": \"{summary}\", \"tail\": \"test result: FAILED\"{extra}}}'\n"
    )
}

/// A check script that runs until it is stopped, and records its pid.
const HANGING_SCRIPT: &str =
    "#!/bin/bash\necho $$ >\"$(dirname \"$0\")/pid\"\nwhile :; do sleep 1; done\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    script_dir: PathBuf,
    pool: sqlx::PgPool,
    db: String,
    bus: EventBus,
    opened: Arc<Mutex<Vec<FixThreadOpening>>>,
    published: Arc<Mutex<Vec<MainCheckSnapshot>>>,
    runner: Arc<RunnerSlot>,
}

async fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_cmd(args, dir).await.unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A maintainer checkout: `main`, the release library and the mirror remote.
async fn fixture(script: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let script_dir = tmp.path().join("check");
    std::fs::create_dir_all(repo.join("scripts/lib")).unwrap();
    std::fs::create_dir_all(&script_dir).unwrap();
    std::fs::write(repo.join("scripts/lib/release_tree.sh"), "# stand-in\n").unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    git(&repo, &["config", "user.email", "test@example.com"]).await;
    git(&repo, &["config", "user.name", "Test"]).await;
    git(&repo, &["add", "."]).await;
    git(&repo, &["commit", "-m", "init"]).await;
    git(
        &repo,
        &[
            "remote",
            "add",
            "lucidos",
            "https://example.invalid/mirror.git",
        ],
    )
    .await;
    write_script(&script_dir, script);
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    Fixture {
        _tmp: tmp,
        repo,
        script_dir,
        pool,
        db,
        bus,
        opened: Arc::default(),
        published: Arc::default(),
        runner: Arc::default(),
    }
}

fn write_script(dir: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("main-check.sh");
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    /// A driver over this fixture, as a fresh engine would build it.
    fn driver(&self) -> (Driver, UnboundedReceiver<MainCheckMsg>) {
        let (tx, rx) = unbounded_channel();
        let opened = self.opened.clone();
        let published = self.published.clone();
        let setup = DriverSetup {
            pool: self.pool.clone(),
            bus: self.bus.clone(),
            repo_root: self.repo.clone(),
            command: MainCheckCommand {
                program: self.script_dir.join("main-check.sh"),
                cwd: self.repo.clone(),
            },
            workspace_name: "myws".into(),
            tx,
            runner: self.runner.clone(),
        };
        let hooks = DriverHooks {
            open_fix_thread: Box::new(move |opening| {
                opened.lock().unwrap().push(opening);
            }),
            publish: Box::new(move |snapshot| published.lock().unwrap().push(snapshot)),
        };
        (Driver::new(setup, hooks), rx)
    }

    /// Commit on `main`, as a landing would. Returns `(main before, main after)`.
    async fn land_commit(&self, file: &str) -> (String, String) {
        let pre = git(&self.repo, &["rev-parse", "main"]).await;
        std::fs::write(self.repo.join(file), file).unwrap();
        git(&self.repo, &["add", file]).await;
        git(&self.repo, &["commit", "-m", &format!("land {file}")]).await;
        (pre, git(&self.repo, &["rev-parse", "main"]).await)
    }

    async fn count(&self, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn latest(&self, event_type: &str) -> serde_json::Value {
        sqlx::query_scalar(
            "SELECT payload->'data' FROM events WHERE event_type = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(event_type)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    fn snapshot(&self) -> MainCheckSnapshot {
        self.published
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    fn shas_checked(&self) -> Vec<String> {
        std::fs::read_to_string(self.script_dir.join("shas"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn landed(thread: Uuid, pre: &str) -> MainCheckMsg {
    MainCheckMsg::Landed {
        thread_id: thread,
        pre_sha: Some(pre.to_string()),
    }
}

/// Hand the driver what its runner sends, through the check's end.
async fn run_to_end(driver: &mut Driver, rx: &mut UnboundedReceiver<MainCheckMsg>) {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("the check ended in time")
            .expect("the runner reports");
        let ended = matches!(msg, MainCheckMsg::Ended { .. });
        driver.handle(msg).await;
        if ended {
            return;
        }
    }
}

/// A landing starts a check of `main` from the landing's base. The script
/// gets that sha, the run shows while it runs, and a pass reads passed.
#[tokio::test]
async fn a_landing_runs_the_check_on_main_and_a_pass_reads_passed() {
    let fx = fixture(&reporting_script("pass", "Every suite passed.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, main) = fx.land_commit("a.txt").await;

    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    let started = fx.latest("MainCheckStarted").await;
    assert_eq!(started["from_sha"], pre.as_str());
    assert_eq!(started["to_sha"], main.as_str());
    assert_eq!(fx.snapshot().state, MainCheckState::Running);

    run_to_end(&mut driver, &mut rx).await;
    assert_eq!(
        fx.shas_checked(),
        vec![main.clone()],
        "the script checked main"
    );
    assert!(
        fx.published
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.state == MainCheckState::Running && s.run_url.as_deref() == Some(RUN_URL)),
        "the running check linked its run"
    );
    assert_eq!(fx.count("MainCheckPassed").await, 1);
    assert_eq!(fx.latest("MainCheckPassed").await["run_url"], RUN_URL);
    let snapshot = fx.snapshot();
    assert_eq!(snapshot.state, MainCheckState::Passed);
    assert_eq!(snapshot.to_sha.as_deref(), Some(main.as_str()));
    teardown_test_db(&fx.db).await;
}

/// Landings while a check runs start nothing, and the pass starts one check
/// covering all of them.
#[tokio::test]
async fn landings_while_a_check_runs_coalesce_into_the_next_check() {
    let fx = fixture(&reporting_script("pass", "Every suite passed.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, first) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    let (b_pre, _) = fx.land_commit("b.txt").await;
    driver.handle(landed(Uuid::new_v4(), &b_pre)).await;
    let (c_pre, last) = fx.land_commit("c.txt").await;
    driver.handle(landed(Uuid::new_v4(), &c_pre)).await;
    assert_eq!(fx.count("MainCheckStarted").await, 1, "one check at a time");

    run_to_end(&mut driver, &mut rx).await;
    assert_eq!(fx.count("MainCheckStarted").await, 2);
    let next = fx.latest("MainCheckStarted").await;
    assert_eq!(next["from_sha"], first.as_str());
    assert_eq!(next["to_sha"], last.as_str());
    run_to_end(&mut driver, &mut rx).await;
    assert_eq!(fx.count("MainCheckPassed").await, 2);
    teardown_test_db(&fx.db).await;
}

/// The failed tests the script names reach the event and the fix thread's
/// prompt, and the raw tail stays out of it.
#[tokio::test]
async fn a_red_carries_its_failed_tests_into_the_event_and_the_prompt() {
    let failures = r#", "failures": [{"suite": "engine", "name": "engine::a::b", "detail": "panicked at a.rs:1"}]"#;
    let fx = fixture(&reporting_script_with("fail", "Failed: engine.", failures)).await;
    let (mut driver, mut rx) = fx.driver();
    let author = Uuid::new_v4();
    start_cc_session(&fx.bus, author, "lucidos-author", None).await;
    let (pre, main) = fx.land_commit("a.txt").await;
    emit_applied(&fx.bus, author, &main, "Fix the parser").await;

    driver.handle(landed(author, &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    let failed = fx.latest("MainCheckFailed").await;
    assert_eq!(failed["failures"][0]["name"], "engine::a::b");
    {
        let opened = fx.opened.lock().unwrap();
        let prompt = &opened[0].prompt;
        assert!(prompt.contains("- engine: engine::a::b\n```\npanicked at a.rs:1\n```"));
        assert!(!prompt.contains("test result: FAILED"), "{prompt}");
    }
    teardown_test_db(&fx.db).await;
}

/// A failure opens exactly one fix thread with what it needs, and no check
/// starts while red is open. A change from the fix thread closes the red and
/// starts the next check.
#[tokio::test]
async fn a_red_opens_one_fix_thread_and_blocks_checks_until_its_fix_lands() {
    let fx = fixture(&reporting_script("fail", "Failed: engine.")).await;
    let (mut driver, mut rx) = fx.driver();
    let author = Uuid::new_v4();
    start_cc_session(&fx.bus, author, "lucidos-author", None).await;
    let (pre, main) = fx.land_commit("a.txt").await;
    emit_applied(&fx.bus, author, &main, "Fix the parser").await;

    driver.handle(landed(author, &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    let failed = fx.latest("MainCheckFailed").await;
    assert_eq!(failed["summary"], "Failed: engine.");
    assert_eq!(failed["tail"], "test result: FAILED");
    let fix_thread: Uuid = serde_json::from_value(failed["fix_thread_id"].clone()).unwrap();
    {
        let opened = fx.opened.lock().unwrap();
        assert_eq!(opened.len(), 1, "exactly one fix thread");
        let FixThreadOpening {
            thread_id: thread,
            prompt,
            title,
            implementer,
            reason,
        } = &opened[0];
        assert_eq!(*implementer, fix_thread_implementer());
        assert_eq!(*reason, EngineReason::MainCheckRed);
        assert_eq!(*thread, fix_thread);
        assert_eq!(title, "Fix main: Failed: engine.");
        for needle in [
            "Failed: engine.",
            RUN_URL,
            "test result: FAILED",
            &format!("{pre}..{main}"),
            &format!("[Fix the parser](thread:myws/{author})"),
        ] {
            assert!(prompt.contains(needle), "missing {needle:?} in:\n{prompt}");
        }
    }
    let snapshot = fx.snapshot();
    assert_eq!(snapshot.state, MainCheckState::Red);
    assert_eq!(snapshot.fix_thread_id, Some(fix_thread));

    let (other_pre, _) = fx.land_commit("b.txt").await;
    driver.handle(landed(Uuid::new_v4(), &other_pre)).await;
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "red blocks new checks"
    );
    assert_eq!(fx.snapshot().state, MainCheckState::Red);

    let (fix_pre, fixed) = fx.land_commit("fix.txt").await;
    driver.handle(landed(fix_thread, &fix_pre)).await;
    let closed = fx.latest("MainCheckRedClosed").await;
    assert_eq!(closed["reason"], "fix_landed");
    assert_eq!(fx.count("MainCheckStarted").await, 2);
    let next = fx.latest("MainCheckStarted").await;
    assert_eq!(next["from_sha"], main.as_str());
    assert_eq!(next["to_sha"], fixed.as_str());
    run_to_end(&mut driver, &mut rx).await;
    assert_eq!(
        fx.opened.lock().unwrap().len(),
        2,
        "the new red has its own fix thread, one per red"
    );
    teardown_test_db(&fx.db).await;
}

/// The maintainer closes the red. A second Close finds nothing to close.
#[tokio::test]
async fn the_maintainer_closes_the_red_and_a_second_close_is_refused() {
    let fx = fixture(&reporting_script("fail", "Failed: api.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    run_to_end(&mut driver, &mut rx).await;

    let actor = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    assert_eq!(close(&mut driver, Some(actor)).await, Ok(()));
    let closed = fx.latest("MainCheckRedClosed").await;
    assert_eq!(closed["reason"], "closed");
    assert_eq!(closed["actor"]["device_id"], "test-device");
    assert_eq!(fx.snapshot().state, MainCheckState::Idle);
    assert_eq!(close(&mut driver, None).await, Err(CloseRefusal::NoRed));
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "nothing landed meanwhile"
    );
    teardown_test_db(&fx.db).await;
}

async fn close(driver: &mut Driver, actor: Option<MessageOrigin>) -> Result<(), CloseRefusal> {
    let (reply, answer) = tokio::sync::oneshot::channel();
    driver.handle(MainCheckMsg::Close { actor, reply }).await;
    answer.await.unwrap()
}

/// GitHub down, or the push refused: the check is abandoned with the
/// script's reason, and it starts nothing by itself. Landings go on.
#[tokio::test]
async fn no_verdict_is_abandoned_and_starts_nothing_by_itself() {
    let fx = fixture(&reporting_script("no-verdict", "the push was refused")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    let (b_pre, _) = fx.land_commit("b.txt").await;
    driver.handle(landed(Uuid::new_v4(), &b_pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    assert_eq!(
        fx.latest("MainCheckAbandoned").await["reason"],
        "the push was refused"
    );
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "an abandoned check starts nothing"
    );
    assert_eq!(fx.snapshot().state, MainCheckState::Idle);

    let (c_pre, last) = fx.land_commit("c.txt").await;
    driver.handle(landed(Uuid::new_v4(), &c_pre)).await;
    let next = fx.latest("MainCheckStarted").await;
    assert_eq!(
        next["from_sha"],
        pre.as_str(),
        "the range still starts at the first unchecked landing"
    );
    assert_eq!(next["to_sha"], last.as_str());
    teardown_test_db(&fx.db).await;
}

/// A script that dies without a verdict line is no verdict, never a pass.
#[tokio::test]
async fn a_script_that_ends_without_a_verdict_is_abandoned() {
    let fx = fixture("#!/bin/bash\necho noise\nexit 3\n").await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    let reason = fx.latest("MainCheckAbandoned").await["reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(reason.contains("without a verdict"), "{reason}");
    assert_eq!(fx.count("MainCheckPassed").await, 0);
    teardown_test_db(&fx.db).await;
}

impl Fixture {
    /// A branch off `main` that adds `file`, returning its head: a change's
    /// snapshot, as the lane would queue it.
    async fn snapshot_adding(&self, file: &str) -> String {
        let branch = format!("lucidos-{}", file.replace('.', "-"));
        let wt = self.script_dir.join(format!("wt-{branch}"));
        let wt_path = wt.to_str().unwrap();
        git(
            &self.repo,
            &["worktree", "add", "-b", &branch, wt_path, "main"],
        )
        .await;
        std::fs::write(wt.join(file), file).unwrap();
        git(&wt, &["add", file]).await;
        git(&wt, &["commit", "-m", &format!("add {file}")]).await;
        let head = git(&wt, &["rev-parse", "HEAD"]).await;
        git(&self.repo, &["worktree", "remove", "--force", wt_path]).await;
        head
    }

    /// Land `snapshot` through the landing lane's clean pass, bounded so a
    /// landing that waited on the check fails the test rather than hangs it.
    /// Returns `main` after it.
    async fn land_through_the_lane(&self, snapshot: &str) -> String {
        use crate::engine::landing_lane::{publish_clean_pass, LandingInput};
        let input = LandingInput {
            change_id: Uuid::new_v4(),
            snapshot_sha: snapshot.to_string(),
            subject: "Land: a change".into(),
            body: "Branch: a branch".into(),
        };
        let published = tokio::time::timeout(
            Duration::from_secs(20),
            publish_clean_pass(
                crate::engine::git_ops::MainLocks::private_for_test(),
                &self.repo,
                &[input],
            ),
        )
        .await
        .expect("the landing waits on nothing")
        .expect("the landing published");
        assert_eq!(published.failed, None);
        assert_ne!(published.main_now(), published.base, "main moved");
        git(&self.repo, &["rev-parse", "main"]).await
    }
}

/// The main check gates nothing. A landing moves `main` while a check runs,
/// and the next check covers it.
#[tokio::test]
async fn a_landing_moves_main_while_a_check_runs() {
    let fx = fixture(HANGING_SCRIPT).await;
    let (mut driver, _rx) = fx.driver();
    let (pre, checked) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    let pid = wait_for_pid(&fx.script_dir.join("pid")).await;
    assert_eq!(fx.snapshot().state, MainCheckState::Running);

    let snapshot = fx.snapshot_adding("b.txt").await;
    let main = fx.land_through_the_lane(&snapshot).await;
    assert!(fx.repo.join("b.txt").exists(), "the landing reached main");
    assert!(process_alive(pid), "the check is still running");
    assert_eq!(fx.snapshot().state, MainCheckState::Running);

    driver.handle(landed(Uuid::new_v4(), &checked)).await;
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "the landing joins the next check rather than starting a second"
    );
    tokio::time::timeout(Duration::from_secs(15), fx.runner.stop())
        .await
        .expect("the check stops in time");
    assert_ne!(main, checked);
    teardown_test_db(&fx.db).await;
}

/// GitHub unreachable: the check script reports no verdict and exits 76. A
/// landing moves `main` all the same, during that check and after it.
#[tokio::test]
async fn a_landing_moves_main_while_github_gives_no_verdict() {
    let script = format!(
        "{}exit 76\n",
        reporting_script("no-verdict", "GitHub is unreachable.")
    );
    let fx = fixture(&script).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    assert_eq!(fx.count("MainCheckStarted").await, 1);

    let during = fx.snapshot_adding("b.txt").await;
    fx.land_through_the_lane(&during).await;
    run_to_end(&mut driver, &mut rx).await;
    assert_eq!(
        fx.latest("MainCheckAbandoned").await["reason"],
        "GitHub is unreachable."
    );
    assert_eq!(fx.snapshot().state, MainCheckState::Idle);

    let after = fx.snapshot_adding("c.txt").await;
    let before_after = git(&fx.repo, &["rev-parse", "main"]).await;
    let main = fx.land_through_the_lane(&after).await;
    assert!(fx.repo.join("b.txt").exists() && fx.repo.join("c.txt").exists());
    driver.handle(landed(Uuid::new_v4(), &before_after)).await;
    let next = fx.latest("MainCheckStarted").await;
    assert_eq!(
        next["from_sha"],
        pre.as_str(),
        "the next check still starts at the first unchecked landing"
    );
    assert_eq!(next["to_sha"], main.as_str());
    tokio::time::timeout(Duration::from_secs(15), fx.runner.stop())
        .await
        .expect("the check stops in time");
    teardown_test_db(&fx.db).await;
}

/// A checkout without the mirror's remote runs no check, and says why.
#[tokio::test]
async fn a_checkout_that_cannot_run_the_check_stays_idle_and_says_why() {
    let fx = fixture(&reporting_script("pass", "ok")).await;
    git(&fx.repo, &["remote", "remove", "lucidos"]).await;
    let (mut driver, _rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    assert_eq!(fx.count("MainCheckStarted").await, 0);
    let snapshot = fx.snapshot();
    assert_eq!(snapshot.state, MainCheckState::Unavailable);
    assert!(snapshot.summary.unwrap().contains("'lucidos' remote"));
    teardown_test_db(&fx.db).await;
}

/// A shutdown before the run is on record stops the child, and the next start
/// abandons the check. Recovery starts nothing by itself.
#[tokio::test]
async fn a_restart_before_the_run_is_linked_stops_the_child_and_abandons_the_check() {
    let fx = fixture(HANGING_SCRIPT).await;
    let (mut driver, _rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    let pid = wait_for_pid(&fx.script_dir.join("pid")).await;

    tokio::time::timeout(Duration::from_secs(15), fx.runner.stop())
        .await
        .expect("shutdown stops the child in time");
    assert!(!process_alive(pid), "the child is gone after shutdown");

    let (mut restarted, _rx) = fx.driver();
    restarted.handle(MainCheckMsg::Restore).await;
    assert_eq!(fx.count("MainCheckAbandoned").await, 1);
    assert_eq!(
        fx.latest("MainCheckAbandoned").await["reason"],
        ENGINE_RESTARTED
    );
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "recovery starts nothing"
    );
    assert_eq!(fx.snapshot().state, MainCheckState::Idle);
    teardown_test_db(&fx.db).await;
}

/// A check script that names its run and then waits. Stopped, it records
/// whether it was told to detach or to cancel. Started with `--attach`, it
/// records its arguments and reports a pass.
const LINKING_SCRIPT: &str = "#!/bin/bash\ndir=\"$(dirname \"$0\")\"\n\
     echo \"$*\" >>\"$dir/args\"\n\
     if [ \"${2:-}\" = --attach ]; then\n\
       echo \"main-check run $3 $4\"\n\
       echo 'main-check verdict {\"verdict\": \"pass\", \"summary\": \"ok\"}'\n\
       exit 0\n\
     fi\n\
     trap 'echo detached >\"$dir/stopped\"; exit 0' USR2\n\
     trap 'echo cancelled >\"$dir/stopped\"; exit 0' TERM\n\
     echo \"main-check run https://example.invalid/actions/runs/7 e2e/20261019-120000-ab12\"\n\
     while :; do sleep 0.1; done\n";

/// A shutdown after the run is on record detaches from it. The next start
/// reattaches to the same run and pushes no second one.
#[tokio::test]
async fn a_restart_after_the_run_is_linked_reattaches_to_the_same_run() {
    let fx = fixture(LINKING_SCRIPT).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, main) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    let msg = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the run line came in time")
        .expect("the runner reports");
    assert!(matches!(msg, MainCheckMsg::RunLinked { .. }));
    driver.handle(msg).await;
    let linked = fx.latest("MainCheckRunLinked").await;
    assert_eq!(linked["run_url"], RUN_URL);
    assert_eq!(linked["branch"], RUN_BRANCH);
    assert_eq!(fx.snapshot().run_url.as_deref(), Some(RUN_URL));

    tokio::time::timeout(Duration::from_secs(15), fx.runner.stop())
        .await
        .expect("shutdown stops the child in time");
    let stopped = std::fs::read_to_string(fx.script_dir.join("stopped")).unwrap();
    assert_eq!(stopped.trim(), "detached", "a linked run is left going");

    let (mut restarted, mut rx) = fx.driver();
    restarted.handle(MainCheckMsg::Restore).await;
    assert_eq!(fx.snapshot().state, MainCheckState::Running);
    run_to_end(&mut restarted, &mut rx).await;
    assert_eq!(fx.count("MainCheckAbandoned").await, 0);
    assert_eq!(fx.count("MainCheckPassed").await, 1);
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "no second run starts"
    );
    assert_eq!(
        fx.count("MainCheckRunLinked").await,
        1,
        "the same run, once"
    );
    let args = std::fs::read_to_string(fx.script_dir.join("args")).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        vec![
            main.clone(),
            format!("{main} --attach {RUN_URL} {RUN_BRANCH}")
        ]
    );
    teardown_test_db(&fx.db).await;
}

/// A checkout that lost what a check needs abandons a linked check rather
/// than reattach, and names the run it left.
#[tokio::test]
async fn a_linked_check_that_cannot_reattach_is_abandoned_with_its_run() {
    let fx = fixture(LINKING_SCRIPT).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    let msg = tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("the run line came in time")
        .expect("the runner reports");
    driver.handle(msg).await;
    tokio::time::timeout(Duration::from_secs(15), fx.runner.stop())
        .await
        .expect("shutdown stops the child in time");

    git(&fx.repo, &["remote", "remove", "lucidos"]).await;
    let (mut restarted, _rx) = fx.driver();
    restarted.handle(MainCheckMsg::Restore).await;
    assert_eq!(fx.count("MainCheckAbandoned").await, 1);
    assert_eq!(fx.latest("MainCheckAbandoned").await["run_url"], RUN_URL);
    assert_eq!(fx.snapshot().state, MainCheckState::Unavailable);
    teardown_test_db(&fx.db).await;
}

/// Shutdown gives the script time to cancel its run and delete its branch.
/// It returns once the script exits, not after a fixed sleep.
#[tokio::test]
async fn a_stopped_child_finishes_its_cleanup_before_shutdown_returns() {
    let tmp = tempfile::tempdir().unwrap();
    write_script(
        tmp.path(),
        "#!/bin/bash\ndir=\"$(dirname \"$0\")\"\n\
         trap 'sleep 1; echo done >\"$dir/cleaned\"; exit 0' TERM\n\
         echo $$ >\"$dir/pid\"\nwhile :; do sleep 0.1; done\n",
    );
    let slot = Arc::new(RunnerSlot::with_grace(Duration::from_secs(8)));
    let (tx, _rx) = unbounded_channel();
    spawn_runner(
        &script_command(tmp.path()),
        Uuid::new_v4(),
        RunTarget::Commit("sha"),
        tx,
        &slot,
    );
    let pid = wait_for_pid(&tmp.path().join("pid")).await;

    let started = std::time::Instant::now();
    slot.stop().await;
    assert!(
        tmp.path().join("cleaned").exists(),
        "the cleanup ran to its end"
    );
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "the stop returned when the script exited, not at the end of its grace"
    );
    assert!(!process_alive(pid));
}

/// A script that ignores SIGTERM is killed once its grace runs out.
#[tokio::test]
async fn a_child_that_ignores_the_stop_is_killed_after_its_grace() {
    let tmp = tempfile::tempdir().unwrap();
    write_script(
        tmp.path(),
        "#!/bin/bash\ntrap '' TERM\necho $$ >\"$(dirname \"$0\")/pid\"\n\
         while :; do sleep 0.1; done\n",
    );
    let slot = Arc::new(RunnerSlot::with_grace(Duration::from_millis(500)));
    let (tx, _rx) = unbounded_channel();
    spawn_runner(
        &script_command(tmp.path()),
        Uuid::new_v4(),
        RunTarget::Commit("sha"),
        tx,
        &slot,
    );
    let pid = wait_for_pid(&tmp.path().join("pid")).await;

    tokio::time::timeout(Duration::from_secs(5), slot.stop())
        .await
        .expect("the stop is bounded by its grace");
    assert!(!process_alive(pid), "the child is gone after its grace");
}

/// The slot holds one child. A second start while one runs starts nothing and
/// ends its check with no verdict, and the running child is left alone.
#[tokio::test]
async fn a_second_child_never_starts_beside_a_running_one() {
    let tmp = tempfile::tempdir().unwrap();
    write_script(tmp.path(), HANGING_SCRIPT);
    let slot = Arc::new(RunnerSlot::with_grace(Duration::from_secs(2)));
    let (tx, mut rx) = unbounded_channel();
    spawn_runner(
        &script_command(tmp.path()),
        Uuid::new_v4(),
        RunTarget::Commit("a"),
        tx.clone(),
        &slot,
    );
    let pid = wait_for_pid(&tmp.path().join("pid")).await;

    let second = Uuid::new_v4();
    spawn_runner(
        &script_command(tmp.path()),
        second,
        RunTarget::Commit("b"),
        tx,
        &slot,
    );
    match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        Ok(Some(MainCheckMsg::Ended { check_id, report })) => {
            assert_eq!(check_id, second);
            assert_eq!(report.verdict, ReportedVerdict::NoVerdict);
        }
        _ => panic!("the second start ends at once with no verdict"),
    }
    assert!(process_alive(pid), "the running child is untouched");
    slot.stop().await;
    assert!(!process_alive(pid));
}

fn script_command(dir: &Path) -> MainCheckCommand {
    MainCheckCommand {
        program: dir.join("main-check.sh"),
        cwd: dir.to_path_buf(),
    }
}

async fn wait_for_pid(file: &Path) -> i32 {
    for _ in 0..100 {
        if let Some(pid) = std::fs::read_to_string(file)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the check script never started");
}

fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// A red survives a restart. Its fix thread opens again only if it never got
/// its prompt, and always under the same id: one fix thread per red.
#[tokio::test]
async fn a_red_survives_a_restart_and_its_fix_thread_opens_once() {
    let fx = fixture(&reporting_script("fail", "Failed: engine.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    let fix_thread = fx.opened.lock().unwrap()[0].thread_id;

    let (mut unprompted, _rx) = fx.driver();
    unprompted.handle(MainCheckMsg::Restore).await;
    assert_eq!(fx.snapshot().state, MainCheckState::Red);
    {
        let opened = fx.opened.lock().unwrap();
        assert_eq!(
            opened.len(),
            2,
            "a fix thread with no events gets its prompt"
        );
        assert_eq!(
            opened[1].thread_id, fix_thread,
            "the same thread, never a second one"
        );
    }

    start_cc_session(&fx.bus, fix_thread, "lucidos-fix", None).await;
    let (mut prompted, _rx) = fx.driver();
    prompted.handle(MainCheckMsg::Restore).await;
    assert_eq!(
        fx.opened.lock().unwrap().len(),
        2,
        "an opened fix thread is left alone"
    );
    assert_eq!(fx.count("MainCheckRedClosed").await, 0);
    teardown_test_db(&fx.db).await;
}

/// Deleting a thread removes its events, so a deleted fix thread looks
/// unprompted. Its `ThreadsDeleted` record keeps a restart from re-opening
/// it, and the red waits on the maintainer's Close.
#[tokio::test]
async fn a_deleted_fix_thread_stays_deleted_across_a_restart() {
    let fx = fixture(&reporting_script("fail", "Failed: engine.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    let fix_thread = fx.opened.lock().unwrap()[0].thread_id;
    fx.bus
        .emit(BusEvent::System(
            crate::engine::event_bus::SystemEvent::ThreadsDeleted {
                thread_ids: vec![fix_thread],
                event_count: 0,
                memory_count: 0,
                worktrees_removed: 0,
                widgets_removed: 0,
                actor: None,
            },
        ))
        .await
        .expect("delete");

    let (mut restarted, _rx) = fx.driver();
    restarted.handle(MainCheckMsg::Restore).await;
    assert_eq!(
        fx.opened.lock().unwrap().len(),
        1,
        "a deleted fix thread never comes back"
    );
    assert_eq!(fx.snapshot().state, MainCheckState::Red);
    assert_eq!(close(&mut restarted, None).await, Ok(()));
    teardown_test_db(&fx.db).await;
}

/// The driver starts before recovery restores it. A landing that reaches it
/// first waits for the restore, so a red from before the restart still holds
/// checks back.
#[tokio::test]
async fn a_landing_before_the_restore_waits_for_it() {
    let fx = fixture(&reporting_script("fail", "Failed: engine.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    assert_eq!(fx.count("MainCheckStarted").await, 1);

    let (mut restarted, _rx) = fx.driver();
    let (b_pre, _) = fx.land_commit("b.txt").await;
    restarted.deliver(landed(Uuid::new_v4(), &b_pre)).await;
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "nothing is handled before the restore"
    );
    restarted.deliver(MainCheckMsg::Restore).await;
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "the landing ran after the restore, and the red held it back"
    );
    let snapshot = fx.snapshot();
    assert_eq!(snapshot.state, MainCheckState::Red);
    assert_eq!(close(&mut restarted, None).await, Ok(()));
    assert_eq!(
        fx.count("MainCheckStarted").await,
        2,
        "the held landing is what the Close starts a check for"
    );
    teardown_test_db(&fx.db).await;
}

/// The fix landed, and a restart came before the red closed: recovery closes
/// it.
#[tokio::test]
async fn a_fix_that_landed_before_a_restart_closes_the_red_on_recovery() {
    let fx = fixture(&reporting_script("fail", "Failed: engine.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    let fix_thread = fx.opened.lock().unwrap()[0].thread_id;
    start_cc_session(&fx.bus, fix_thread, "lucidos-fix", None).await;
    let (_, fixed) = fx.land_commit("fix.txt").await;
    emit_applied(&fx.bus, fix_thread, &fixed, "Fix main").await;

    let (mut restarted, _rx) = fx.driver();
    restarted.handle(MainCheckMsg::Restore).await;
    assert_eq!(
        fx.latest("MainCheckRedClosed").await["reason"],
        "fix_landed"
    );
    assert_eq!(fx.snapshot().state, MainCheckState::Idle);
    teardown_test_db(&fx.db).await;
}

/// Only a `ChangeApplied` with a landing merge is a landing. An app change,
/// a no-op apply and every other event are not.
#[test]
fn only_a_landing_merge_reads_as_a_landing() {
    let thread = Uuid::new_v4();
    let applied = |landing: Option<&str>| BusEvent::Thread {
        thread_id: thread,
        event: applied_event(Uuid::new_v4(), landing, None),
        meta: EventMeta::NONE,
    };
    match landing_of(&applied(Some("m1"))) {
        Some(MainCheckMsg::Landed { thread_id, pre_sha }) => {
            assert_eq!(thread_id, thread);
            assert_eq!(pre_sha.as_deref(), Some("pre"));
        }
        _ => panic!("a landing merge is a landing"),
    }
    assert!(landing_of(&applied(None)).is_none());
    assert!(landing_of(&BusEvent::System(
        crate::engine::event_bus::SystemEvent::TreeBackfillReset {}
    ))
    .is_none());
}

/// The feed's replay list gates live frames too, so a landing event it omits
/// would never reach the driver.
#[test]
fn the_landing_feed_replays_the_event_a_landing_comes_from() {
    let landing = applied_event(Uuid::new_v4(), Some("m1"), None);
    assert!(LANDING_EVENT_TYPES.contains(&landing.event_type()));
}

/// A lag drops no landing: the feed replays it from the events table, with
/// the thread and the base a fix landing needs to close its red.
#[tokio::test]
async fn a_landing_reaches_the_main_check_through_a_lag() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fix_thread = Uuid::new_v4();
    start_cc_session(&bus, fix_thread, "lucidos-fix", None).await;
    let (mut rx, mut replay) = LagReplay::subscribe(
        &bus,
        ReplayScope::Workspace,
        Some(LANDING_EVENT_TYPES.to_vec()),
    );
    emit_applied(&bus, fix_thread, "m1", "Fix main").await;
    flood_past_bus_capacity(&bus);

    let landings = Mutex::new(Vec::new());
    assert!(
        forward_landings(rx.recv().await, &mut replay, &pool, |msg| {
            if let MainCheckMsg::Landed { thread_id, pre_sha } = msg {
                landings.lock().unwrap().push((thread_id, pre_sha));
            }
        })
        .await
    );
    assert_eq!(
        landings.into_inner().unwrap(),
        vec![(fix_thread, Some("pre".to_string()))]
    );
    teardown_test_db(&db).await;
}

impl Fixture {
    /// Make `main` unreadable, as a git read that timed out leaves it.
    fn hide_main(&self) {
        let git_dir = self.repo.join(".git");
        std::fs::rename(git_dir.join("refs/heads/main"), git_dir.join("main-hidden")).unwrap();
    }

    fn unhide_main(&self) {
        let git_dir = self.repo.join(".git");
        std::fs::rename(git_dir.join("main-hidden"), git_dir.join("refs/heads/main")).unwrap();
    }

    /// Make the events table unreadable, as a failed recovery query finds it.
    async fn hide_events(&self) {
        sqlx::query("ALTER TABLE events RENAME TO events_hidden")
            .execute(&self.pool)
            .await
            .unwrap();
    }

    async fn unhide_events(&self) {
        sqlx::query("ALTER TABLE events_hidden RENAME TO events")
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

/// Git cannot read `main` as a pass comes in. The landing during the run is
/// still checked, from `main` as the landing read it.
#[tokio::test]
async fn a_pass_while_git_cannot_read_main_still_checks_what_landed_during_it() {
    let fx = fixture(&reporting_script("pass", "Every suite passed.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, checked) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    let (b_pre, last) = fx.land_commit("b.txt").await;
    driver.handle(landed(Uuid::new_v4(), &b_pre)).await;

    fx.hide_main();
    run_to_end(&mut driver, &mut rx).await;
    fx.unhide_main();
    assert_eq!(
        fx.count("MainCheckStarted").await,
        2,
        "the landing during the run gets its check"
    );
    let next = fx.latest("MainCheckStarted").await;
    assert_eq!(next["from_sha"], checked.as_str());
    assert_eq!(next["to_sha"], last.as_str());
    run_to_end(&mut driver, &mut rx).await;
    teardown_test_db(&fx.db).await;
}

/// Git cannot read `main` as the fix lands. The red closes all the same, no
/// check starts on an unread `main`, and the next landing's check covers the
/// fix.
#[tokio::test]
async fn a_fix_landing_while_git_cannot_read_main_still_closes_the_red() {
    let fx = fixture(&reporting_script("fail", "Failed: engine.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, red_to) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    let fix_thread = fx.opened.lock().unwrap()[0].thread_id;

    let (fix_pre, _) = fx.land_commit("fix.txt").await;
    fx.hide_main();
    driver.handle(landed(fix_thread, &fix_pre)).await;
    fx.unhide_main();
    assert_eq!(
        fx.latest("MainCheckRedClosed").await["reason"],
        "fix_landed"
    );
    assert_eq!(fx.count("MainCheckStarted").await, 1);
    assert_eq!(fx.snapshot().state, MainCheckState::Idle);

    let (next_pre, last) = fx.land_commit("b.txt").await;
    driver.handle(landed(Uuid::new_v4(), &next_pre)).await;
    let next = fx.latest("MainCheckStarted").await;
    assert_eq!(
        next["from_sha"],
        red_to.as_str(),
        "the check covers the fix"
    );
    assert_eq!(next["to_sha"], last.as_str());
    teardown_test_db(&fx.db).await;
}

/// The recovery query fails. Landings keep waiting, each retries the restore,
/// and once it succeeds the red from before the restart holds them back.
#[tokio::test]
async fn a_failed_restore_holds_landings_until_a_retry_restores_the_red() {
    let fx = fixture(&reporting_script("fail", "Failed: engine.")).await;
    let (mut driver, mut rx) = fx.driver();
    let (pre, _) = fx.land_commit("a.txt").await;
    driver.handle(landed(Uuid::new_v4(), &pre)).await;
    run_to_end(&mut driver, &mut rx).await;
    let (b_pre, _) = fx.land_commit("b.txt").await;
    let (c_pre, last) = fx.land_commit("c.txt").await;

    let (mut restarted, _rx) = fx.driver();
    fx.hide_events().await;
    let (early_reply, early_answer) = tokio::sync::oneshot::channel();
    restarted
        .deliver(MainCheckMsg::Close {
            actor: None,
            reply: early_reply,
        })
        .await;
    restarted.deliver(MainCheckMsg::Restore).await;
    assert_eq!(
        early_answer.await.unwrap(),
        Err(CloseRefusal::StateUnread),
        "a Close held for the first restore answers when that restore fails"
    );
    restarted.deliver(landed(Uuid::new_v4(), &b_pre)).await;
    let (reply, answer) = tokio::sync::oneshot::channel();
    restarted
        .deliver(MainCheckMsg::Close { actor: None, reply })
        .await;
    assert_eq!(
        answer.await.unwrap(),
        Err(CloseRefusal::StateUnread),
        "a Close answers at once rather than wait for the next landing"
    );
    fx.unhide_events().await;
    restarted.deliver(landed(Uuid::new_v4(), &c_pre)).await;
    assert_eq!(
        fx.count("MainCheckStarted").await,
        1,
        "the retried restore found the red, and it held both landings back"
    );
    assert_eq!(fx.snapshot().state, MainCheckState::Red);
    assert_eq!(close(&mut restarted, None).await, Ok(()));
    assert_eq!(fx.count("MainCheckStarted").await, 2);
    assert_eq!(fx.latest("MainCheckStarted").await["to_sha"], last.as_str());
    teardown_test_db(&fx.db).await;
}

/// The main check is fed by the bus and gates nothing: the landing lane, which
/// moves `main`, never names it.
#[test]
fn the_landing_lane_never_waits_on_the_main_check() {
    use crate::test_support::source_scan::production_sources;
    let coupled: Vec<String> = production_sources()
        .into_iter()
        .filter(|(rel, source)| {
            rel.starts_with("engine/landing_lane/")
                && (source.contains("main_check") || source.contains("MainCheck"))
        })
        .map(|(rel, _)| rel)
        .collect();
    assert!(
        coupled.is_empty(),
        "the lane reaches the main check: {coupled:?}"
    );
}

fn applied_event(change_id: Uuid, landing: Option<&str>, title: Option<&str>) -> ThreadEvent {
    ThreadEvent::ChangeApplied {
        change_id: change_id.to_string(),
        requires_restart: false,
        client_update: false,
        commits: vec![],
        thread_title: title.map(str::to_string),
        actor: None,
        pre_merge_sha: landing.map(|_| "pre".to_string()),
        post_merge_sha: landing.map(str::to_string),
        landing_merge_sha: landing.map(str::to_string),
        path: String::new(),
    }
}

/// Record that `thread`'s change landed as `landing`.
async fn emit_applied(bus: &EventBus, thread: Uuid, landing: &str, title: &str) {
    let change_id = Uuid::new_v4();
    let meta = || EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::ChangeProposed {
            change_id: change_id.to_string(),
            description: Some("work".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: format!("lucidos-{thread}"),
            repo_root: "/repo".into(),
            hardened: true,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: meta(),
    })
    .await
    .expect("propose");
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: applied_event(change_id, Some(landing), Some(title)),
        meta: meta(),
    })
    .await
    .expect("apply");
}

const FLAKY_TESTS: &str =
    r#", "flaky": [{"suite": "engine", "name": "engine::a::b", "detail": "panicked at a.rs:1"}]"#;

/// Land one more commit as `thread` and let its check run to its end.
async fn land_and_check(
    fx: &Fixture,
    driver: &mut Driver,
    rx: &mut UnboundedReceiver<MainCheckMsg>,
    thread: Uuid,
    file: &str,
) {
    let (pre, _) = fx.land_commit(file).await;
    driver.handle(landed(thread, &pre)).await;
    run_to_end(driver, rx).await;
}

/// A fixture whose checks pass with one flaky test each.
async fn flaky_fixture() -> Fixture {
    fixture(&reporting_script_with(
        "pass",
        "Every suite passed.",
        FLAKY_TESTS,
    ))
    .await
}

/// A pass with flaky tests opens one flaky fix thread, framed as flaky, and
/// holds no check. While it is open, a later pass records its flaky tests and
/// opens nothing.
#[tokio::test]
async fn a_pass_with_flaky_tests_opens_one_flaky_fix_thread_and_blocks_nothing() {
    let fx = flaky_fixture().await;
    let (mut driver, mut rx) = fx.driver();
    land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "a.txt").await;

    let passed = fx.latest("MainCheckPassed").await;
    assert_eq!(passed["flaky"][0]["name"], "engine::a::b");
    let flaky_thread: Uuid = serde_json::from_value(passed["flaky_fix_thread_id"].clone()).unwrap();
    {
        let opened = fx.opened.lock().unwrap();
        assert_eq!(opened.len(), 1, "one flaky fix thread");
        assert_eq!(opened[0].thread_id, flaky_thread);
        assert_eq!(opened[0].reason, EngineReason::MainCheckFlaky);
        assert_eq!(opened[0].implementer, fix_thread_implementer());
        assert!(opened[0].title.starts_with("Fix flaky: engine::a::b"));
        for needle in [
            "passed on main",
            "flaky",
            "engine: engine::a::b",
            "panicked at a.rs:1",
        ] {
            assert!(
                opened[0].prompt.contains(needle),
                "missing {needle:?} in:\n{}",
                opened[0].prompt
            );
        }
    }
    let snapshot = fx.snapshot();
    assert_eq!(snapshot.state, MainCheckState::Passed);
    assert_eq!(snapshot.flaky_fix_thread_id, Some(flaky_thread));

    land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "b.txt").await;
    assert_eq!(
        fx.count("MainCheckStarted").await,
        2,
        "a flaky pass holds no check"
    );
    let again = fx.latest("MainCheckPassed").await;
    assert_eq!(again["flaky"][0]["name"], "engine::a::b");
    assert!(
        again.get("flaky_fix_thread_id").is_none(),
        "no second thread: {again}"
    );
    assert_eq!(fx.opened.lock().unwrap().len(), 1);
    teardown_test_db(&fx.db).await;
}

/// A change from the flaky fix thread closes it. The next flaky pass then
/// opens a new one.
#[tokio::test]
async fn a_landing_from_the_flaky_thread_closes_it() {
    let fx = flaky_fixture().await;
    let (mut driver, mut rx) = fx.driver();
    land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "a.txt").await;
    let flaky_thread = fx.opened.lock().unwrap()[0].thread_id;

    land_and_check(&fx, &mut driver, &mut rx, flaky_thread, "b.txt").await;
    assert_eq!(
        fx.opened.lock().unwrap().len(),
        2,
        "the pass after it opens a new one"
    );
    assert_ne!(fx.opened.lock().unwrap()[1].thread_id, flaky_thread);
    assert_eq!(
        fx.snapshot().flaky_fix_thread_id,
        Some(fx.opened.lock().unwrap()[1].thread_id)
    );
    teardown_test_db(&fx.db).await;
}

/// A restart opens an unprompted flaky fix thread once, under its own id, and
/// leaves a prompted one alone. A second flaky thread never appears.
#[tokio::test]
async fn a_restart_prompts_an_unprompted_flaky_thread_once() {
    let fx = flaky_fixture().await;
    let (mut driver, mut rx) = fx.driver();
    land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "a.txt").await;
    let flaky_thread = fx.opened.lock().unwrap()[0].thread_id;

    let (mut unprompted, _rx) = fx.driver();
    unprompted.handle(MainCheckMsg::Restore).await;
    {
        let opened = fx.opened.lock().unwrap();
        assert_eq!(opened.len(), 2, "no events yet, so it gets its prompt");
        assert_eq!(opened[1].thread_id, flaky_thread, "the same thread");
        assert_eq!(opened[1].reason, EngineReason::MainCheckFlaky);
    }
    assert_eq!(fx.snapshot().flaky_fix_thread_id, Some(flaky_thread));

    start_cc_session(&fx.bus, flaky_thread, "lucidos-flaky", None).await;
    let (mut prompted, _rx) = fx.driver();
    prompted.handle(MainCheckMsg::Restore).await;
    assert_eq!(
        fx.opened.lock().unwrap().len(),
        2,
        "a prompted thread is left alone"
    );
    teardown_test_db(&fx.db).await;
}

/// Hand `old` off to a legacy *successor thread*, as handoffs did before
/// successor sessions: the old thread archives itself and the successor row
/// links back to it.
async fn hand_off(fx: &Fixture, old: Uuid) -> Uuid {
    let successor = Uuid::new_v4();
    start_cc_session(&fx.bus, old, "lucidos-old", None).await;
    start_cc_session(&fx.bus, successor, "lucidos-successor", None).await;
    for (row, column, other) in [
        (successor, "successor_of", old),
        (old, "successor_thread_id", successor),
    ] {
        sqlx::query(&format!(
            "UPDATE thread_summaries SET {column} = $2 WHERE thread_id = $1"
        ))
        .bind(row)
        .bind(other)
        .execute(&fx.pool)
        .await
        .expect("link");
    }
    fx.bus
        .emit(BusEvent::Thread {
            thread_id: old,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await
        .expect("archive");
    successor
}

/// A fix thread that hands off lands its fix under the successor's id. The
/// landing still counts for the fix thread, so the red closes.
#[tokio::test]
async fn a_fix_landed_by_a_successor_closes_the_red() {
    let fx = fixture(&reporting_script("fail", "Failed: engine.")).await;
    let (mut driver, mut rx) = fx.driver();
    land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "a.txt").await;
    let fix_thread = fx.opened.lock().unwrap()[0].thread_id;
    let successor = hand_off(&fx, fix_thread).await;

    let (pre, _) = fx.land_commit("fix.txt").await;
    driver.handle(landed(successor, &pre)).await;
    assert_eq!(
        fx.latest("MainCheckRedClosed").await["reason"],
        "fix_landed"
    );

    let (mut restarted, _rx) = fx.driver();
    restarted.handle(MainCheckMsg::Restore).await;
    assert_ne!(fx.snapshot().state, MainCheckState::Red);
    teardown_test_db(&fx.db).await;
}

/// A flaky fix thread that archived itself for a successor is still open. A
/// later flaky pass opens nothing, and the successor's landing closes it.
#[tokio::test]
async fn a_flaky_fix_thread_that_handed_off_stays_open_until_its_successor_lands() {
    let fx = flaky_fixture().await;
    let (mut driver, mut rx) = fx.driver();
    land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "a.txt").await;
    let flaky_thread = fx.opened.lock().unwrap()[0].thread_id;
    let successor = hand_off(&fx, flaky_thread).await;

    land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "b.txt").await;
    assert_eq!(fx.opened.lock().unwrap().len(), 1, "no second flaky thread");

    let (pre, _) = fx.land_commit("fix.txt").await;
    driver.handle(landed(successor, &pre)).await;
    assert!(fx.snapshot().flaky_fix_thread_id.is_none());
    teardown_test_db(&fx.db).await;
}

/// A flaky fix thread the maintainer deleted or archived stays closed across a
/// restart, and the next flaky pass opens another.
#[tokio::test]
async fn a_deleted_or_archived_flaky_thread_is_closed() {
    for archived in [false, true] {
        let fx = flaky_fixture().await;
        let (mut driver, mut rx) = fx.driver();
        land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "a.txt").await;
        let flaky_thread = fx.opened.lock().unwrap()[0].thread_id;
        if archived {
            start_cc_session(&fx.bus, flaky_thread, "lucidos-flaky", None).await;
            fx.bus
                .emit(BusEvent::Thread {
                    thread_id: flaky_thread,
                    event: ThreadEvent::ThreadArchived,
                    meta: EventMeta::NONE,
                })
                .await
                .expect("archive");
        } else {
            // A delete removes the thread's events, and leaves only this record.
            fx.bus
                .emit(BusEvent::System(
                    crate::engine::event_bus::SystemEvent::ThreadsDeleted {
                        thread_ids: vec![flaky_thread],
                        event_count: 0,
                        memory_count: 0,
                        worktrees_removed: 0,
                        widgets_removed: 0,
                        actor: None,
                    },
                ))
                .await
                .expect("delete");
        }

        let (mut restarted, mut rx) = fx.driver();
        restarted.handle(MainCheckMsg::Restore).await;
        assert_eq!(fx.opened.lock().unwrap().len(), 1, "it never comes back");
        assert_eq!(fx.snapshot().flaky_fix_thread_id, None);

        land_and_check(&fx, &mut restarted, &mut rx, Uuid::new_v4(), "b.txt").await;
        {
            let opened = fx.opened.lock().unwrap();
            assert_eq!(opened.len(), 2, "the next flaky pass opens another");
            assert_ne!(opened[1].thread_id, flaky_thread);
        }
        teardown_test_db(&fx.db).await;
    }
}

/// A failed check is red as before, flaky tests or not: one fix thread, and no
/// flaky one.
#[tokio::test]
async fn a_red_with_flaky_tests_opens_only_its_fix_thread() {
    let fx = fixture(&reporting_script_with(
        "fail",
        "Failed: engine.",
        FLAKY_TESTS,
    ))
    .await;
    let (mut driver, mut rx) = fx.driver();
    land_and_check(&fx, &mut driver, &mut rx, Uuid::new_v4(), "a.txt").await;
    {
        let opened = fx.opened.lock().unwrap();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].reason, EngineReason::MainCheckRed);
    }
    assert_eq!(fx.snapshot().state, MainCheckState::Red);
    teardown_test_db(&fx.db).await;
}
