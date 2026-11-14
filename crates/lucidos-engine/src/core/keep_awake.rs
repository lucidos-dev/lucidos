//! Keeps the computer awake while this engine has work in flight.
//!
//! Each piece of work holds an [`AwakeHold`] for as long as it runs. While one
//! or more holds live, the process holds one `PreventUserIdleSystemSleep`
//! assertion, the same as `caffeinate -i`. Lid close, an explicit Sleep and a
//! thermal emergency still win. The assertion is named after the work, so
//! `pmset -g assertions` says what Lucidos is waiting on.
//!
//! Why the engine owns it, and the alternatives turned down:
//! `docs/adr/0366-the-engine-keeps-the-computer-awake-while-work-runs.md`.

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

/// A kind of work that keeps the computer awake while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Work {
    /// A chat-pipeline turn: chat, an intent trigger, a sub-thread.
    ThreadTurn,
    /// A Claude Code or Codex session, from spawn to exit.
    AgentSession,
    /// A background bash, python or engine-run task.
    BackgroundTask,
    /// A Thread Queue entry's work, script triggers included.
    QueueEntry,
    Backup,
}

impl Work {
    fn counted(self, n: usize) -> String {
        let noun = match self {
            Work::ThreadTurn => "thread turn",
            Work::AgentSession => "coding-agent session",
            Work::BackgroundTask => "background task",
            Work::QueueEntry => "Thread Queue entry",
            Work::Backup => "backup",
        };
        format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
    }
}

/// Takes, renames and releases the one assertion. Split out so tests can
/// record what the registry asked for.
trait Assertion: Send {
    fn take(&mut self, name: &str) -> Result<(), String>;
    fn rename(&mut self, name: &str);
    fn release(&mut self);
}

struct State {
    holds: BTreeMap<u64, (Work, String)>,
    next_id: u64,
    /// Names the assertion, so `pmset` tells one workspace from another.
    workspace: String,
    assertion: Box<dyn Assertion>,
    held: bool,
    /// A platform with no assertion says so once, not on every hold.
    warned: bool,
}

impl State {
    fn name(&self) -> String {
        let mut counts: BTreeMap<Work, usize> = BTreeMap::new();
        for (work, _) in self.holds.values() {
            *counts.entry(*work).or_default() += 1;
        }
        let work: Vec<String> = counts.into_iter().map(|(w, n)| w.counted(n)).collect();
        format!("Lucidos ({}): {}", self.workspace, work.join(", "))
    }
}

/// The process-wide registry of holds. One engine process serves one
/// workspace, so one registry per process is one per workspace.
pub struct KeepAwake {
    state: Mutex<State>,
}

static GLOBAL: LazyLock<Arc<KeepAwake>> =
    LazyLock::new(|| Arc::new(KeepAwake::with_assertion(platform_assertion())));

/// Hold the computer awake until the returned value drops.
///
/// `label` names this piece of work in the log, such as a thread id.
pub fn hold(work: Work, label: impl Into<String>) -> AwakeHold {
    GLOBAL.acquire(work, label.into())
}

/// Name the assertion after this engine's workspace. Called once at boot.
pub fn set_workspace(name: &str) {
    GLOBAL.lock().workspace = name.to_string();
}

/// Whether a hold with this label is live in the process registry. Lets a work
/// site's test check its own hold while other tests hold theirs in parallel.
#[cfg(test)]
pub(crate) fn is_held(label: &str) -> bool {
    GLOBAL.lock().holds.values().any(|(_, l)| l == label)
}

impl KeepAwake {
    fn with_assertion(assertion: Box<dyn Assertion>) -> Self {
        Self {
            state: Mutex::new(State {
                holds: BTreeMap::new(),
                next_id: 0,
                workspace: "workspace".to_string(),
                assertion,
                held: false,
                warned: false,
            }),
        }
    }

    /// The registry's state. A panic while holding the lock leaves a map that
    /// is still consistent, so a poisoned lock is recovered, not propagated.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn acquire(self: &Arc<Self>, work: Work, label: String) -> AwakeHold {
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        state.holds.insert(id, (work, label.clone()));
        let name = state.name();
        if state.held {
            state.assertion.rename(&name);
        } else {
            match state.assertion.take(&name) {
                Ok(()) => {
                    state.held = true;
                    crate::log!(
                        "[KeepAwake] Holding the computer awake for {:?} {} ({})",
                        work,
                        label,
                        name
                    );
                }
                Err(e) if !state.warned => {
                    state.warned = true;
                    crate::log!("[KeepAwake] Cannot keep the computer awake: {}", e);
                }
                Err(_) => {}
            }
        }
        AwakeHold {
            id,
            registry: self.clone(),
        }
    }

    fn release(&self, id: u64) {
        let mut state = self.lock();
        let Some((work, label)) = state.holds.remove(&id) else {
            return;
        };
        if !state.held {
            return;
        }
        if state.holds.is_empty() {
            state.assertion.release();
            state.held = false;
            crate::log!(
                "[KeepAwake] Released: the last work in flight, {:?} {}, finished",
                work,
                label
            );
        } else {
            let name = state.name();
            state.assertion.rename(&name);
        }
    }
}

/// Keeps the computer awake until dropped. Hold it in the value or future that
/// lives exactly as long as the work, so every exit path releases it.
#[must_use = "the computer may sleep as soon as the hold drops"]
pub struct AwakeHold {
    id: u64,
    registry: Arc<KeepAwake>,
}

impl std::fmt::Debug for AwakeHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwakeHold").field("id", &self.id).finish()
    }
}

impl Drop for AwakeHold {
    fn drop(&mut self) {
        self.registry.release(self.id);
    }
}

#[cfg(target_os = "macos")]
fn platform_assertion() -> Box<dyn Assertion> {
    Box::new(iokit::IoKitAssertion::default())
}

#[cfg(not(target_os = "macos"))]
fn platform_assertion() -> Box<dyn Assertion> {
    struct Unsupported;
    impl Assertion for Unsupported {
        fn take(&mut self, _name: &str) -> Result<(), String> {
            Err("this platform has no idle-sleep assertion".to_string())
        }
        fn rename(&mut self, _name: &str) {}
        fn release(&mut self) {}
    }
    Box::new(Unsupported)
}

/// The IOKit power assertion API, called directly so the assertion carries
/// Lucidos's own name. A `caffeinate` child would show as `caffeinate`.
#[cfg(target_os = "macos")]
mod iokit {
    use super::Assertion;
    use std::ffi::{c_char, c_void, CString};

    type CFStringRef = *const c_void;
    type IOPMAssertionID = u32;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
    const K_IO_RETURN_SUCCESS: i32 = 0;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut IOPMAssertionID,
        ) -> i32;
        fn IOPMAssertionSetProperty(
            id: IOPMAssertionID,
            key: CFStringRef,
            value: *const c_void,
        ) -> i32;
        fn IOPMAssertionRelease(id: IOPMAssertionID) -> i32;
    }

    /// An owned CFString, released on drop.
    struct CfString(CFStringRef);

    impl CfString {
        fn new(s: &str) -> Option<Self> {
            let c = CString::new(s.replace('\0', "")).ok()?;
            // SAFETY: `c` is a valid NUL-terminated UTF-8 string that outlives
            // the call, and a null allocator selects the default one.
            let cf = unsafe {
                CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8)
            };
            (!cf.is_null()).then_some(Self(cf))
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            // SAFETY: `self.0` came from a successful Create call and is
            // released exactly once, here.
            unsafe { CFRelease(self.0) }
        }
    }

    #[derive(Default)]
    pub(super) struct IoKitAssertion {
        id: Option<IOPMAssertionID>,
    }

    impl Assertion for IoKitAssertion {
        fn take(&mut self, name: &str) -> Result<(), String> {
            let kind = CfString::new("PreventUserIdleSystemSleep").ok_or("CFString failed")?;
            let name = CfString::new(name).ok_or("CFString failed")?;
            let mut id: IOPMAssertionID = 0;
            // SAFETY: both CFStrings are live for the call, and `id` is a valid
            // out-pointer.
            let rc = unsafe {
                IOPMAssertionCreateWithName(kind.0, K_IOPM_ASSERTION_LEVEL_ON, name.0, &mut id)
            };
            if rc != K_IO_RETURN_SUCCESS {
                return Err(format!("IOPMAssertionCreateWithName returned {rc:#x}"));
            }
            self.id = Some(id);
            Ok(())
        }

        fn rename(&mut self, name: &str) {
            let (Some(id), Some(key), Some(name)) =
                (self.id, CfString::new("AssertName"), CfString::new(name))
            else {
                return;
            };
            // SAFETY: `id` is a live assertion this value created, and both
            // CFStrings are live for the call. A failed rename keeps the old
            // name, which is still a true description of held work.
            unsafe { IOPMAssertionSetProperty(id, key.0, name.0) };
        }

        fn release(&mut self) {
            if let Some(id) = self.id.take() {
                // SAFETY: `id` is a live assertion this value created, released
                // exactly once because `take` cleared it.
                unsafe { IOPMAssertionRelease(id) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the registry asked of the assertion, in order.
    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Take(String),
        Rename(String),
        Release,
    }

    struct Recording {
        calls: Arc<Mutex<Vec<Call>>>,
        fail: bool,
    }

    impl Assertion for Recording {
        fn take(&mut self, name: &str) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Take(name.to_string()));
            if self.fail {
                Err("no assertion here".to_string())
            } else {
                Ok(())
            }
        }
        fn rename(&mut self, name: &str) {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Rename(name.to_string()));
        }
        fn release(&mut self) {
            self.calls.lock().unwrap().push(Call::Release);
        }
    }

    fn registry(fail: bool) -> (Arc<KeepAwake>, Arc<Mutex<Vec<Call>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let assertion = Recording {
            calls: calls.clone(),
            fail,
        };
        let registry = Arc::new(KeepAwake::with_assertion(Box::new(assertion)));
        registry.lock().workspace = "dev".to_string();
        (registry, calls)
    }

    #[test]
    fn the_first_hold_takes_and_the_last_drop_releases() {
        let (registry, calls) = registry(false);
        let turn = registry.acquire(Work::ThreadTurn, "t1".into());
        let task = registry.acquire(Work::BackgroundTask, "b1".into());
        drop(turn);
        drop(task);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                Call::Take("Lucidos (dev): 1 thread turn".into()),
                Call::Rename("Lucidos (dev): 1 thread turn, 1 background task".into()),
                Call::Rename("Lucidos (dev): 1 background task".into()),
                Call::Release,
            ]
        );
    }

    #[test]
    fn the_name_counts_each_kind_of_work() {
        let (registry, calls) = registry(false);
        let _a = registry.acquire(Work::ThreadTurn, "t1".into());
        let _b = registry.acquire(Work::ThreadTurn, "t2".into());
        let _c = registry.acquire(Work::Backup, "backup".into());
        assert_eq!(
            calls.lock().unwrap().last(),
            Some(&Call::Rename(
                "Lucidos (dev): 2 thread turns, 1 backup".into()
            ))
        );
    }

    /// Idle, then busy again: a fresh assertion, not a rename of a released one.
    #[test]
    fn work_after_idle_takes_a_new_assertion() {
        let (registry, calls) = registry(false);
        drop(registry.acquire(Work::Backup, "backup".into()));
        drop(registry.acquire(Work::AgentSession, "s1".into()));
        let takes = calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| matches!(c, Call::Take(_)))
            .count();
        assert_eq!(takes, 2);
        assert_eq!(calls.lock().unwrap().last(), Some(&Call::Release));
    }

    /// No assertion on this platform: work still gets its hold, and nothing is
    /// released that was never taken.
    #[test]
    fn a_platform_without_an_assertion_still_hands_out_holds() {
        let (registry, calls) = registry(true);
        let hold = registry.acquire(Work::QueueEntry, "q1".into());
        assert!(registry.lock().warned);
        drop(hold);
        assert!(!calls.lock().unwrap().contains(&Call::Release));
        assert!(registry.lock().holds.is_empty());
    }

    /// A panic unwinding through the work still drops its hold.
    #[test]
    fn a_panic_releases_the_hold() {
        let (registry, calls) = registry(false);
        let held = registry.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _hold = held.acquire(Work::ThreadTurn, "t1".into());
            panic!("the work failed");
        }));
        assert!(result.is_err());
        assert_eq!(calls.lock().unwrap().last(), Some(&Call::Release));
        assert!(registry.lock().holds.is_empty());
    }

    #[test]
    fn the_process_registry_tracks_a_hold_by_label() {
        let label = format!("test-{}", uuid::Uuid::new_v4());
        let held = hold(Work::BackgroundTask, label.clone());
        assert!(is_held(&label));
        drop(held);
        assert!(!is_held(&label));
    }
}
