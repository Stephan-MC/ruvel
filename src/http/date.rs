//! Lock-free cached RFC 9110 IMF-fixdate (`Date` response header) clock.
//!
//! Uses a hybrid architecture:
//! - When at least one `Application::serve` instance is running (`ACTIVE_TICKERS > 0`), a
//!   whole-second-aligned Tokio background task refreshes an `ArcSwap<[u8; 37]>` so request
//!   serialization reads the formatted `Date` header line wait-free with zero syscalls.
//! - Outside `Application::serve` (e.g. unit tests or standalone `Response::to_bytes()`), falls
//!   back to a thread-local cache refreshed at most once per second.

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

/// Exact wire byte length of `"Date: Www, DD Mmm YYYY HH:MM:SS GMT\r\n"`.
pub(crate) const DATE_HEADER_LEN: usize = 37;

/// Preformatted IMF-fixdate HTTP header line including trailing `\r\n`.
pub(crate) type DateHeaderLine = [u8; DATE_HEADER_LEN];

static SHARED_DATE: OnceLock<ArcSwap<DateHeaderLine>> = OnceLock::new();
static LAST_SHARED_SECS: AtomicU64 = AtomicU64::new(u64::MAX);
static ACTIVE_TICKERS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static TL_DATE: Cell<(u64, DateHeaderLine)> =
        const { Cell::new((u64::MAX, *b"Date: Thu, 01 Jan 1970 00:00:00 GMT\r\n")) };
}

/// RAII guard for the background clock task spawned by `Application::serve`.
pub(crate) struct DateTickerGuard {
    handle: Option<JoinHandle<()>>,
}

impl Drop for DateTickerGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            ACTIVE_TICKERS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[inline]
fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn refresh_shared(secs: u64) {
    let cache = SHARED_DATE.get_or_init(|| ArcSwap::from_pointee(format_date_header(secs)));
    if LAST_SHARED_SECS.swap(secs, Ordering::AcqRel) != secs {
        cache.store(std::sync::Arc::new(format_date_header(secs)));
    }
}

/// Starts a background whole-second-aligned Tokio task keeping the shared `Date` cache current.
///
/// The task is aborted and `ACTIVE_TICKERS` is decremented when the returned guard drops.
pub(crate) fn start_server_ticker() -> DateTickerGuard {
    refresh_shared(unix_timestamp_secs());
    let handle = Handle::try_current().ok().map(|rt| {
        rt.spawn(async move {
            loop {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default();
                refresh_shared(now.as_secs());
                let nanos_to_next_sec =
                    u64::from(1_000_000_000u32.saturating_sub(now.subsec_nanos())) + 1_000_000;
                tokio::time::sleep(Duration::from_nanos(nanos_to_next_sec)).await;
            }
        })
    });
    if handle.is_some() {
        ACTIVE_TICKERS.fetch_add(1, Ordering::Release);
    }
    DateTickerGuard { handle }
}

/// Passes the current formatted `Date` header line (`"Date: ... GMT\r\n"`) to `f`.
#[inline]
pub(crate) fn with_current_date_header<R>(f: impl FnOnce(&DateHeaderLine) -> R) -> R {
    if ACTIVE_TICKERS.load(Ordering::Acquire) > 0
        && let Some(cache) = SHARED_DATE.get()
    {
        let guard = cache.load();
        return f(&guard);
    }
    with_thread_local_date_header(f)
}

#[cold]
#[inline(never)]
fn with_thread_local_date_header<R>(f: impl FnOnce(&DateHeaderLine) -> R) -> R {
    let now = unix_timestamp_secs();
    TL_DATE.with(|cell| {
        let (cached_secs, mut line) = cell.get();
        if cached_secs != now {
            line = format_date_header(now);
            cell.set((now, line));
        }
        f(&line)
    })
}

#[inline]
fn write_two_digits(dst: &mut [u8], offset: usize, val: u8) {
    dst[offset] = b'0' + (val / 10);
    dst[offset + 1] = b'0' + (val % 10);
}

/// Formats a UTC Unix timestamp into an RFC 9110 §5.6.7 IMF-fixdate header line.
pub(crate) fn format_date_header(unix_secs: u64) -> DateHeaderLine {
    const WEEKDAYS: [[u8; 3]; 7] = [
        *b"Sun", *b"Mon", *b"Tue", *b"Wed", *b"Thu", *b"Fri", *b"Sat",
    ];
    const MONTHS: [[u8; 3]; 12] = [
        *b"Jan", *b"Feb", *b"Mar", *b"Apr", *b"May", *b"Jun", *b"Jul", *b"Aug", *b"Sep", *b"Oct",
        *b"Nov", *b"Dec",
    ];

    let days = (unix_secs / 86_400) as i64;
    let sod = (unix_secs % 86_400) as u32;

    // Howard Hinnant `civil_from_days` algorithm (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (y + i64::from(month <= 2)).clamp(0, 9999) as u32;

    // 1970-01-01 was a Thursday (index 4).
    let weekday = WEEKDAYS[((days + 4).rem_euclid(7)) as usize];
    let month_name = MONTHS[(month - 1) as usize];

    let hour = (sod / 3600) as u8;
    let minute = ((sod % 3600) / 60) as u8;
    let second = (sod % 60) as u8;

    let mut out = *b"Date: Thu, 01 Jan 1970 00:00:00 GMT\r\n";
    out[6..9].copy_from_slice(&weekday);
    write_two_digits(&mut out, 11, day as u8);
    out[14..17].copy_from_slice(&month_name);
    write_two_digits(&mut out, 18, (year / 100) as u8);
    write_two_digits(&mut out, 20, (year % 100) as u8);
    write_two_digits(&mut out, 23, hour);
    write_two_digits(&mut out, 26, minute);
    write_two_digits(&mut out, 29, second);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_utc_timestamps_as_imf_fixdate() {
        assert_eq!(
            &format_date_header(0),
            b"Date: Thu, 01 Jan 1970 00:00:00 GMT\r\n"
        );
        assert_eq!(
            &format_date_header(951_782_400),
            b"Date: Tue, 29 Feb 2000 00:00:00 GMT\r\n"
        );
        assert_eq!(
            &format_date_header(1_709_251_199),
            b"Date: Thu, 29 Feb 2024 23:59:59 GMT\r\n"
        );
        assert_eq!(
            &format_date_header(1_735_689_599),
            b"Date: Tue, 31 Dec 2024 23:59:59 GMT\r\n"
        );
        assert_eq!(
            &format_date_header(4_107_542_400),
            b"Date: Mon, 01 Mar 2100 00:00:00 GMT\r\n"
        );
    }

    #[tokio::test]
    async fn server_ticker_guard_manages_active_count_and_serves_valid_header() {
        let before = ACTIVE_TICKERS.load(Ordering::Acquire);
        let guard = start_server_ticker();
        assert_eq!(ACTIVE_TICKERS.load(Ordering::Acquire), before + 1);

        let line = with_current_date_header(|bytes| *bytes);
        assert_eq!(line.len(), DATE_HEADER_LEN);
        assert!(line.starts_with(b"Date: "));
        assert!(line.ends_with(b" GMT\r\n"));

        drop(guard);
        assert_eq!(ACTIVE_TICKERS.load(Ordering::Acquire), before);
    }
}
