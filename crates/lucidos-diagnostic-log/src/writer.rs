use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::{sanitize, Line};

pub const CHANNEL_CAPACITY: usize = 4096;

pub const SEGMENT_MAX_BYTES: u64 = 16 * 1024 * 1024;

pub const FILE_NAME: &str = "diagnostic-log.jsonl";

pub const ROTATED_FILE_NAME: &str = "diagnostic-log.jsonl.1";

const OWN_LABEL: &str = "DiagnosticLog";

pub enum Entry {
    Record(Pending),
    Flush(std::sync::mpsc::Sender<()>),
}

pub struct Pending {
    ts: DateTime<Utc>,
    label: &'static str,
    msg: String,
    cut: bool,
    line: Option<Line>,
}

impl Pending {
    fn classified(label: &'static str, line: Line) -> Self {
        let line = line.bounded();
        let msg = line.render(super::Destination::Owner);
        let (msg, cut) = sanitize::bound_raw(msg);
        Self {
            ts: Utc::now(),
            label,
            msg,
            cut,
            line: Some(line),
        }
    }

    fn into_record(self) -> Record {
        let mut record = Record::new(self.ts, self.label, &self.msg, self.cut);
        record.line = self.line;
        record
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub ts: String,
    pub pid: u32,
    pub label: String,
    pub msg: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<Line>,
}

impl Record {
    pub fn render(&self, destination: super::Destination) -> String {
        match &self.line {
            Some(line) => line.render(destination),
            None if destination == super::Destination::Maintainer => "<unstructured line>".into(),
            None => sanitize::for_read(&self.msg),
        }
    }

    fn new(ts: DateTime<Utc>, label: &str, msg: &str, cut: bool) -> Self {
        Self {
            ts: ts.to_rfc3339_opts(SecondsFormat::Millis, true),
            pid: std::process::id(),
            label: label.to_string(),
            msg: sanitize::for_write(msg, cut),
            line: None,
        }
    }
}

pub struct Sink {
    tx: SyncSender<Entry>,
    dropped: Arc<AtomicU64>,
}

impl Sink {
    pub fn flush(&self, timeout: std::time::Duration) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        self.tx.try_send(Entry::Flush(tx)).is_ok() && rx.recv_timeout(timeout).is_ok()
    }

    pub fn record_line(&self, label: &'static str, line: Line) {
        let entry = Entry::Record(Pending::classified(label, line));
        if let Err(TrySendError::Full(_)) = self.tx.try_send(entry) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record(&self, label: &'static str, msg: String) {
        let (msg, cut) = sanitize::bound_raw(msg);
        let entry = Entry::Record(Pending {
            ts: Utc::now(),
            label,
            msg,
            cut,
            line: None,
        });
        if let Err(TrySendError::Full(_)) = self.tx.try_send(entry) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn channel(capacity: usize) -> (Sink, Receiver<Entry>, Arc<AtomicU64>) {
    let (tx, rx) = sync_channel(capacity);
    let dropped = Arc::new(AtomicU64::new(0));
    let sink = Sink {
        tx,
        dropped: dropped.clone(),
    };
    (sink, rx, dropped)
}

pub fn ensure_real_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if std::fs::symlink_metadata(dir)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} is a symlink or not a directory",
            dir.display()
        )))
    }
}

pub fn open_no_follow(path: &Path, options: &mut std::fs::OpenOptions) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

fn open_for_append(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    open_no_follow(path, &mut options)
}

pub struct Segments {
    dir: PathBuf,
    file: File,
    size: u64,
}

impl Segments {
    pub fn open(dir: &Path) -> io::Result<Self> {
        ensure_real_dir(dir)?;
        let file = open_for_append(&dir.join(FILE_NAME))?;
        let size = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            size,
        })
    }

    pub fn append_line(&mut self, label: &'static str, line: Line) -> io::Result<()> {
        let record = Pending::classified(label, line).into_record();
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        self.append(&bytes)
    }

    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        let len = bytes.len() as u64;
        if self.unlinked()? {
            self.reopen()?;
        } else if self.size > 0 && self.size + len > SEGMENT_MAX_BYTES {
            self.rotate()?;
        }
        self.file.write_all(bytes)?;
        self.size += len;
        Ok(())
    }

    #[cfg(unix)]
    fn unlinked(&self) -> io::Result<bool> {
        use std::os::unix::fs::MetadataExt;
        Ok(self.file.metadata()?.nlink() == 0)
    }

    #[cfg(not(unix))]
    fn unlinked(&self) -> io::Result<bool> {
        Ok(false)
    }

    fn rotate(&mut self) -> io::Result<()> {
        ensure_real_dir(&self.dir)?;
        match std::fs::rename(self.dir.join(FILE_NAME), self.dir.join(ROTATED_FILE_NAME)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        self.reopen()
    }

    fn reopen(&mut self) -> io::Result<()> {
        ensure_real_dir(&self.dir)?;
        self.file = open_for_append(&self.dir.join(FILE_NAME))?;
        self.size = self.file.metadata()?.len();
        Ok(())
    }
}

pub fn drain(rx: Receiver<Entry>, mut segments: Segments, dropped: &AtomicU64) {
    let mut failing = false;
    for entry in rx {
        let entry = match entry {
            Entry::Record(entry) => entry,
            Entry::Flush(done) => {
                let _ = done.send(());
                continue;
            }
        };
        let mut records = Vec::with_capacity(2);
        let lost = dropped.swap(0, Ordering::Relaxed);
        if lost > 0 {
            records.push(Record::new(
                entry.ts,
                OWN_LABEL,
                &format!("dropped {lost} lines"),
                false,
            ));
        }
        records.push(entry.into_record());
        let mut bytes = Vec::new();
        for record in &records {
            if serde_json::to_writer(&mut bytes, record).is_ok() {
                bytes.push(b'\n');
            }
        }
        match segments.append(&bytes) {
            Ok(()) => failing = false,
            Err(e) if !failing => {
                failing = true;
                crate::stderr::host_error(&format!(
                    "[DiagnosticLog] cannot append to the diagnostic log: {e}"
                ));
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(dir: &Path, name: &str) -> Vec<Record> {
        std::fs::read_to_string(dir.join(name))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn an_entry_lands_as_one_sanitized_json_line() {
        let tmp = tempfile::tempdir().unwrap();
        let (sink, rx, dropped) = channel(8);
        sink.record(
            "Voice",
            "token ghp_Ab3dEf6hIj9kLm2nOp5qRs8tUv1wXy4zAb7d\nnext".into(),
        );
        drop(sink);
        drain(rx, Segments::open(tmp.path()).unwrap(), &dropped);
        let lines = records(tmp.path(), FILE_NAME);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].label, "Voice");
        assert_eq!(lines[0].pid, std::process::id());
        assert_eq!(lines[0].msg, "token ghp_[REDACTED]\nnext");
        assert!(lines[0].ts.ends_with('Z'), "{}", lines[0].ts);
        chrono::DateTime::parse_from_rfc3339(&lines[0].ts).unwrap();
    }

    #[test]
    fn a_full_channel_drops_the_line_and_the_writer_records_the_count() {
        let tmp = tempfile::tempdir().unwrap();
        let (sink, rx, dropped) = channel(1);
        sink.record("A", "kept".into());
        sink.record("A", "lost one".into());
        sink.record("A", "lost two".into());
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        let writer_dropped = dropped.clone();
        let writer = std::thread::spawn(move || {
            drain(rx, Segments::open(tmp.path()).unwrap(), &writer_dropped);
            tmp
        });
        drop(sink);
        let tmp = writer.join().unwrap();
        let msgs: Vec<String> = records(tmp.path(), FILE_NAME)
            .into_iter()
            .map(|r| format!("{} {}", r.label, r.msg))
            .collect();
        assert_eq!(msgs, ["DiagnosticLog dropped 2 lines", "A kept"]);
    }

    #[test]
    fn a_segment_rotates_into_one_older_segment() {
        let tmp = tempfile::tempdir().unwrap();
        let mut segments = Segments::open(tmp.path()).unwrap();
        let line = vec![b'x'; 1024 * 1024];
        for _ in 0..40 {
            segments.append(&line).unwrap();
        }
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, [FILE_NAME, ROTATED_FILE_NAME]);
        for name in &names {
            let len = std::fs::metadata(tmp.path().join(name)).unwrap().len();
            assert!(len <= SEGMENT_MAX_BYTES, "{name} is {len} bytes");
        }
    }

    #[test]
    fn a_deleted_or_moved_segment_is_reopened() {
        let tmp = tempfile::tempdir().unwrap();
        let mut segments = Segments::open(tmp.path()).unwrap();
        segments.append(b"before\n").unwrap();
        std::fs::remove_file(tmp.path().join(FILE_NAME)).unwrap();
        segments.append(b"after delete\n").unwrap();
        let current = tmp.path().join(FILE_NAME);
        assert_eq!(std::fs::read_to_string(&current).unwrap(), "after delete\n");

        std::fs::rename(&current, tmp.path().join("moved")).unwrap();
        segments.size = SEGMENT_MAX_BYTES;
        segments.append(b"after move\n").unwrap();
        assert_eq!(std::fs::read_to_string(&current).unwrap(), "after move\n");
    }

    #[cfg(unix)]
    #[test]
    fn the_writer_refuses_a_symlinked_dir_or_file() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("target.log"), "untouched").unwrap();

        let linked_dir = tmp.path().join("linked-dir");
        std::os::unix::fs::symlink(&outside, &linked_dir).unwrap();
        assert!(Segments::open(&linked_dir).is_err());

        let real_dir = tmp.path().join("real-dir");
        std::fs::create_dir(&real_dir).unwrap();
        std::os::unix::fs::symlink(outside.join("target.log"), real_dir.join(FILE_NAME)).unwrap();
        assert!(Segments::open(&real_dir).is_err());
        assert_eq!(
            std::fs::read_to_string(outside.join("target.log")).unwrap(),
            "untouched"
        );
    }
}
