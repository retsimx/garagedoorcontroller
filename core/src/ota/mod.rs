//! Pure OTA policy: version decision, URL/version/hash parsing, Basic auth, the
//! byte-fed HTTP response-head parser, and the streaming verified update
//! session. No hardware, no allocation.
//!
//! The byte-fed head parser and the streaming session live in cohesive
//! submodules and are re-exported here, so callers keep the flat
//! `garagedoor_core::ota::*` surface:
//!
//! - [`http`] — the response-head parser,
//! - [`session`] — the streaming [`apply_update`] session.

#![allow(async_fn_in_trait)]

mod http;
pub mod selftest;
mod session;

pub use http::{HeadError, HeadEvent, HeadParser, ResponseHead};
pub use selftest::{evaluate, SelfTestDecision, SelfTestSignals, SELF_TEST_WINDOW_MS};
pub use session::{apply_update, BodyReader, Flasher, UpdateError};

/// Streaming chunk size for the OTA body (4 KiB, matching the RP2040 flash page).
pub const CHUNK_BYTES: usize = 4096;

/// Fixed-capacity storage for the assembled URL path (`/prefix/project`).
const PATH_BYTES: usize = 192;

/// Whether a remote version should replace the local one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    /// Fetch and apply the remote image.
    Update,
    /// Keep the running image.
    Skip,
}

/// Decides whether to update. Any difference (including a rollback) updates.
pub fn decide(local: u32, remote: u32) -> Decision {
    if remote != local {
        Decision::Update
    } else {
        Decision::Skip
    }
}

/// Why a base URL plus project could not be turned into a request [`Uri`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UrlError {
    /// Missing or unsupported scheme (only `http`/`https` are accepted).
    Scheme,
    /// Missing or malformed host.
    Host,
    /// Malformed or zero port.
    Port,
    /// Missing or malformed project name.
    Project,
    /// Path prefix too long or containing invalid bytes.
    Path,
}

/// A parsed request target: scheme, authority and a fixed-capacity path.
#[derive(Debug)]
pub struct Uri<'a> {
    /// `true` for `https`, `false` for `http`.
    pub tls: bool,
    /// Host name or literal IPv4 address.
    pub host: &'a str,
    /// TCP port (443/80 when omitted).
    pub port: u16,
    path: [u8; PATH_BYTES],
    path_len: usize,
}

impl Uri<'_> {
    /// The assembled path, e.g. `/prefix/project`.
    pub fn path(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
}

/// Parses `base` (`scheme://host[:port][/prefix...]`) and appends `project` to
/// the path. Host and path bytes are restricted to `[A-Za-z0-9._-]`.
pub fn parse_url<'a>(base: &'a str, project: &'a str) -> Result<Uri<'a>, UrlError> {
    let (scheme, rest) = base.split_once("://").ok_or(UrlError::Scheme)?;
    let tls = match scheme {
        "https" => true,
        "http" => false,
        _ => return Err(UrlError::Scheme),
    };
    if !valid_segment(project) {
        return Err(UrlError::Project);
    }
    let (authority, prefix) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = parse_authority(authority, tls)?;

    let mut path = [0u8; PATH_BYTES];
    let mut len = 0;
    for segment in prefix.split('/').filter(|s| !s.is_empty()).chain([project]) {
        if len + 1 + segment.len() > PATH_BYTES || !valid_segment(segment) {
            return Err(UrlError::Path);
        }
        path[len] = b'/';
        len += 1;
        path[len..len + segment.len()].copy_from_slice(segment.as_bytes());
        len += segment.len();
    }
    Ok(Uri {
        tls,
        host,
        port,
        path,
        path_len: len,
    })
}

fn parse_authority(authority: &str, tls: bool) -> Result<(&str, u16), UrlError> {
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|_| UrlError::Port)?;
            if port == 0 {
                return Err(UrlError::Port);
            }
            (host, port)
        }
        None => (authority, if tls { 443 } else { 80 }),
    };
    if host.is_empty() || !host.bytes().all(valid_host_byte) {
        return Err(UrlError::Host);
    }
    Ok((host, port))
}

fn valid_host_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_'
}

fn valid_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(valid_host_byte)
}

/// Parses a trimmed bare decimal integer, rejecting overflow.
pub fn parse_version(body: &[u8]) -> Option<u32> {
    let s = trim_ascii(body);
    if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value: u32 = 0;
    for &b in s {
        value = value.checked_mul(10)?.checked_add(u32::from(b - b'0'))?;
    }
    Some(value)
}

/// Parses exactly 64 lowercase hex characters (optional surrounding whitespace)
/// into a 32-byte SHA-256 digest.
pub fn parse_sha256_hex(body: &[u8]) -> Option<[u8; 32]> {
    let s = trim_ascii(body);
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = hex_nibble(s[2 * i])?;
        let lo = hex_nibble(s[2 * i + 1])?;
        *byte = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Base64-encodes `input` into `out`, returning the encoded length.
///
/// Returns `None` if `out` is too small; never writes past `out`.
pub fn base64(input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut bytes = input.iter().copied();
    base64_fill(|| bytes.next(), out)
}

/// Writes `Basic <base64(user:pass)>` into `out`, returning the header length.
///
/// Returns `None` when both credentials are empty (no header should be sent) or
/// when `out` is too small.
pub fn basic_authorization(user: &str, pass: &str, out: &mut [u8]) -> Option<usize> {
    const PREFIX: &[u8] = b"Basic ";
    if user.is_empty() && pass.is_empty() {
        return None;
    }
    if out.len() < PREFIX.len() {
        return None;
    }
    out[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut bytes = user
        .bytes()
        .chain(core::iter::once(b':'))
        .chain(pass.bytes());
    let n = base64_fill(|| bytes.next(), &mut out[PREFIX.len()..])?;
    Some(PREFIX.len() + n)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_fill<F: FnMut() -> Option<u8>>(mut next: F, out: &mut [u8]) -> Option<usize> {
    let mut o = 0;
    loop {
        let b0 = match next() {
            Some(byte) => byte,
            None => return Some(o),
        };
        let b1 = next();
        let b2 = next();
        if out.len() < o + 4 {
            return None;
        }
        let n =
            (u32::from(b0) << 16) | (u32::from(b1.unwrap_or(0)) << 8) | u32::from(b2.unwrap_or(0));
        out[o] = B64[((n >> 18) & 63) as usize];
        out[o + 1] = B64[((n >> 12) & 63) as usize];
        out[o + 2] = match b1 {
            Some(_) => B64[((n >> 6) & 63) as usize],
            None => b'=',
        };
        out[o + 3] = match b2 {
            Some(_) => B64[(n & 63) as usize],
            None => b'=',
        };
        o += 4;
        if b2.is_none() {
            return Some(o);
        }
    }
}

/// Trims ASCII whitespace from both ends of `input`.
pub(crate) fn trim_ascii(input: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = input.len();
    while start < end && input[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && input[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &input[start..end]
}

#[cfg(test)]
mod tests;
