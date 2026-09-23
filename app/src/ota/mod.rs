//! OTA transport and task glue (GDC-7, retsimx/garagedoorcontroller#8).
//!
//! Pure policy (URL/version/hash parsing, the byte-fed HTTP head parser and the
//! streaming verified update session) lives in `garagedoor_core::ota`. This
//! module owns only the hardware/network glue, split for cohesion:
//!
//! - [`transport`] — DNS, TCP (`embassy-net`), TLS 1.3 (`embedded-tls`, no-op
//!   verifier), the HTTP/1.0 GET request/response and the body adapter,
//! - [`task`] — the `ota_task` and the `check_and_update` orchestration.
//!
//! The task runs one check at boot (after DHCP) unless the bootloader just
//! reverted an image, then one per
//! [`crate::telemetry::RESET_REQUEST_SIGNAL`] notification; there is no periodic
//! poll. The control path is never blocked: every network wait is a
//! timeout-wrapped await, and flash is touched only after a response head and a
//! hash are accepted.

mod task;
mod transport;

use embassy_executor::Spawner;

use crate::radio::NetStack;
use crate::update;

use task::ota_task;

/// Spawn the OTA task. The task owns `updater` for the life of the program and
/// only touches flash after a response head and a hash have been accepted.
/// `boot_check_allowed` is `false` when the bootloader just reverted an image,
/// which suppresses the automatic boot check and breaks the update loop; a
/// manual `garagedoor/reset` still checks.
pub fn spawn(
    spawner: Spawner,
    stack: NetStack,
    updater: update::Updater,
    boot_check_allowed: bool,
) {
    spawner.spawn(defmt::unwrap!(ota_task(stack, updater, boot_check_allowed)));
}

/// A fixed-capacity `core::fmt::Write` sink for requests and short names.
struct TextBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> TextBuf<N> {
    fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

impl<const N: usize> core::fmt::Write for TextBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > N {
            return Err(core::fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Transport-level failure categories. The underlying `Error` types are
/// intentionally dropped: logging them defensively keeps secrets out and lets
/// one error type serve both plain and TLS connections.
#[derive(Clone, Copy)]
enum FetchError {
    Timeout,
    Write,
    Flush,
    Read,
    Closed,
    Head(&'static str),
    Leftover,
    BodyTooLarge,
}

impl FetchError {
    fn code(self) -> &'static str {
        match self {
            FetchError::Timeout => "timeout",
            FetchError::Write => "write",
            FetchError::Flush => "flush",
            FetchError::Read => "read",
            FetchError::Closed => "closed",
            FetchError::Head(code) => code,
            FetchError::Leftover => "leftover",
            FetchError::BodyTooLarge => "body_too_large",
        }
    }
}

/// Coarse reason an OTA check did not complete; logged as `code=`.
enum OtaError {
    Url,
    Dns,
    Connect,
    Tls,
    Timeout,
    Request,
    Auth,
    Head,
    Version,
    Sha,
    Transport(FetchError),
    Update(&'static str),
}

impl OtaError {
    fn code(&self) -> &'static str {
        match self {
            OtaError::Url => "url",
            OtaError::Dns => "dns",
            OtaError::Connect => "connect",
            OtaError::Tls => "tls",
            OtaError::Timeout => "timeout",
            OtaError::Request => "request",
            OtaError::Auth => "auth",
            OtaError::Head => "head",
            OtaError::Version => "version",
            OtaError::Sha => "sha",
            OtaError::Transport(error) => error.code(),
            OtaError::Update(code) => code,
        }
    }

    /// LED blink code reporting where an OTA check failed, counted on the
    /// onboard beacon. OTA codes start at 10 so they never collide with the
    /// Wi-Fi beacon codes 1/2/3. A human reports the count when no probe or
    /// device log is available.
    fn blink_code(&self) -> u8 {
        match self {
            OtaError::Dns => 10,
            OtaError::Connect => 11,
            OtaError::Tls => 12,
            OtaError::Timeout => 13,
            OtaError::Url
            | OtaError::Request
            | OtaError::Auth
            | OtaError::Head
            | OtaError::Version => 14,
            OtaError::Sha => 15,
            OtaError::Update(code) => match *code {
                "hash_mismatch" => 16,
                "too_short" => 17,
                "too_long" => 18,
                "flash" => 19,
                "read" => 20,
                "oversize" => 21,
                _ => 14,
            },
            OtaError::Transport(FetchError::Head(_)) => 14,
            OtaError::Transport(_) => 22,
        }
    }
}
