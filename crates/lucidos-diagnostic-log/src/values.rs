use std::fmt;

/// Implement only for types whose complete formatted value is shareable.
/// Text, paths and error displays require [`private`].
pub trait LogValue {
    fn is_private(&self) -> bool {
        false
    }
}

pub fn check_value<T: LogValue + ?Sized>(_: &T) {}

impl<T: LogValue + ?Sized> LogValue for &T {
    fn is_private(&self) -> bool {
        T::is_private(self)
    }
}

macro_rules! shareable {
    ($($ty:ty),* $(,)?) => { $(impl LogValue for $ty {})* };
}
shareable!(
    bool,
    char,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    f32,
    f64,
    uuid::Uuid,
    std::time::Duration,
    std::io::ErrorKind,
    http::StatusCode,
    chrono::DateTime<chrono::Utc>
);

pub struct Private<T>(T);

pub fn private<T>(value: T) -> Private<T> {
    Private(value)
}

impl<T> LogValue for Private<T> {
    fn is_private(&self) -> bool {
        true
    }
}

impl<T: fmt::Display> fmt::Display for Private<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
impl<T: fmt::Debug> fmt::Debug for Private<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// An error exposes its typed kind separately from its private display text.
pub trait ErrorKind {
    type Kind: LogValue;
    fn log_kind(&self) -> Self::Kind;
}
impl ErrorKind for std::io::Error {
    type Kind = std::io::ErrorKind;
    fn log_kind(&self) -> Self::Kind {
        self.kind()
    }
}
