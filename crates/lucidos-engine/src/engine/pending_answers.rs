//! Requests a page answers: an app capture, a widget fit check. The engine
//! emits a thread event scoped to one device, then waits here for that
//! device's answer. Allowed-ephemeral: a restart drops the waiting tool call
//! with it.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::oneshot;

/// A request waiting for its answer.
pub struct PendingAnswer<T> {
    /// The device that must answer. `None` when any page may.
    target: Option<String>,
    tx: oneshot::Sender<T>,
}

/// The requests waiting for an answer, by request id.
pub type PendingAnswers<T> = Mutex<HashMap<String, PendingAnswer<T>>>;

/// Why an answer was not taken.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AnswerRefusal {
    /// No request waits under this id: it timed out, or was already answered.
    Unknown,
    /// The request waits for another device. It stays open for that one.
    OtherDevice,
}

/// Start waiting under `request_id` for `target`'s answer, or any page's.
pub(crate) fn await_answer<T>(
    pending: &PendingAnswers<T>,
    request_id: &str,
    target: Option<&str>,
) -> oneshot::Receiver<T> {
    let (tx, rx) = oneshot::channel();
    pending.lock().unwrap().insert(
        request_id.to_string(),
        PendingAnswer {
            target: target.map(str::to_string),
            tx,
        },
    );
    rx
}

/// Stop waiting under `request_id`: a timeout, or a request never sent.
pub(crate) fn forget_answer<T>(pending: &PendingAnswers<T>, request_id: &str) {
    pending.lock().unwrap().remove(request_id);
}

/// Hand `answer` to the request `request_id` waits on, if `answering` is the
/// device it waits for.
pub(crate) fn resolve_answer<T>(
    pending: &PendingAnswers<T>,
    request_id: &str,
    answering: Option<&str>,
    answer: T,
) -> Result<(), AnswerRefusal> {
    let mut waiting = pending.lock().unwrap();
    let Entry::Occupied(request) = waiting.entry(request_id.to_string()) else {
        return Err(AnswerRefusal::Unknown);
    };
    let target = request.get().target.as_deref();
    if target.is_some_and(|target| answering != Some(target)) {
        return Err(AnswerRefusal::OtherDevice);
    }
    // A closed receiver means the tool call itself is gone (a canceled turn),
    // so there is nobody left to tell.
    let _ = request.remove().tx.send(answer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_target_device_resolves_a_request() {
        let pending: PendingAnswers<u8> = Mutex::default();
        let mut rx = await_answer(&pending, "r1", Some("phone"));
        assert_eq!(
            resolve_answer(&pending, "r1", Some("laptop"), 1),
            Err(AnswerRefusal::OtherDevice)
        );
        assert_eq!(
            resolve_answer(&pending, "r1", None, 1),
            Err(AnswerRefusal::OtherDevice)
        );
        assert!(rx.try_recv().is_err(), "the request stays open");
        assert_eq!(resolve_answer(&pending, "r1", Some("phone"), 2), Ok(()));
        assert_eq!(rx.try_recv(), Ok(2));
        assert_eq!(
            resolve_answer(&pending, "r1", Some("phone"), 3),
            Err(AnswerRefusal::Unknown),
            "an answered request takes no second answer"
        );
    }

    #[test]
    fn any_page_resolves_a_request_with_no_target() {
        let pending: PendingAnswers<u8> = Mutex::default();
        let mut rx = await_answer(&pending, "r1", None);
        assert_eq!(resolve_answer(&pending, "r1", Some("laptop"), 7), Ok(()));
        assert_eq!(rx.try_recv(), Ok(7));
    }

    #[test]
    fn a_forgotten_request_takes_no_answer() {
        let pending: PendingAnswers<u8> = Mutex::default();
        let _rx = await_answer(&pending, "r1", None);
        forget_answer(&pending, "r1");
        assert_eq!(
            resolve_answer(&pending, "r1", None, 1),
            Err(AnswerRefusal::Unknown)
        );
    }
}
