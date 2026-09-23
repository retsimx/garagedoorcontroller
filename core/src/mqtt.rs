//! Pure, `no_std` MQTT payload codec.
//!
//! This module owns the MQTT wire contract topics and the minimal JSON payload
//! grammar used by the telemetry task. It has zero dependencies and zero heap
//! allocation: parsing borrows the caller's payload, and formatting writes into
//! a caller-supplied stack buffer. The scanner is intentionally structurally
//! aware (brace/bracket/string aware) rather than a full JSON library, so that
//! a `uuid` key nested inside a sub-object is never mistaken for a top-level key.

/// Topic carrying a door trigger command.
pub const TOPIC_TRIGGER: &str = "garagedoor/trigger";
/// Topic carrying a status query request.
pub const TOPIC_STATUS: &str = "garagedoor/status";
/// Topic carrying an OTA reset request.
pub const TOPIC_RESET: &str = "garagedoor/reset";
/// Topic carrying door state-change notifications.
pub const TOPIC_ONCHANGE: &str = "garagedoor/onchange";
/// Topic carrying the correlated status query response.
pub const TOPIC_STATUS_RESPONSE: &str = "garagedoor/status/response";

/// Reason a `garagedoor/status` request payload was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusRequestError {
    /// The payload is not a JSON object.
    NotObject,
    /// The object has no top-level `uuid` key.
    MissingUuid,
    /// The top-level `uuid` key is present but its value is not a JSON string.
    UuidNotString,
    /// The top-level `uuid` value is an empty string.
    EmptyUuid,
    /// The payload is structurally invalid JSON.
    Malformed,
}

/// Reason a payload could not be encoded into the caller buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// The caller-supplied buffer was too small for the encoded payload.
    Truncated,
}

/// Extracts the top-level `uuid` string value from a status request payload.
///
/// The returned `&str` borrows `payload` and is the raw string token body
/// (escape sequences are honoured when locating the closing quote but are not
/// decoded). The first top-level `uuid` key wins.
pub fn parse_status_request(payload: &[u8]) -> Result<&str, StatusRequestError> {
    let mut scanner = Scanner {
        bytes: payload,
        pos: 0,
    };

    scanner.skip_ws();
    if scanner.peek() != Some(b'{') {
        return Err(StatusRequestError::NotObject);
    }
    scanner.advance();
    scanner.skip_ws();

    let mut found: Option<&str> = None;
    if scanner.peek() == Some(b'}') {
        scanner.advance();
    } else {
        loop {
            scanner.skip_ws();
            if scanner.peek() != Some(b'"') {
                return Err(StatusRequestError::Malformed);
            }
            let key = scanner.parse_string_raw()?;

            scanner.skip_ws();
            if scanner.peek() != Some(b':') {
                return Err(StatusRequestError::Malformed);
            }
            scanner.advance();

            let value = scanner.parse_value()?;
            if key == "uuid" && found.is_none() {
                match value {
                    Value::String(uuid) => found = Some(uuid),
                    Value::Other => return Err(StatusRequestError::UuidNotString),
                }
            }

            scanner.skip_ws();
            match scanner.peek() {
                Some(b',') => scanner.advance(),
                Some(b'}') => {
                    scanner.advance();
                    break;
                }
                _ => return Err(StatusRequestError::Malformed),
            }
        }
    }

    scanner.skip_ws();
    if scanner.pos != scanner.bytes.len() {
        return Err(StatusRequestError::Malformed);
    }

    match found {
        Some("") => Err(StatusRequestError::EmptyUuid),
        Some(uuid) => Ok(uuid),
        None => Err(StatusRequestError::MissingUuid),
    }
}

/// Writes the compact `{"open":<bool>}` state notification into `buf`.
pub fn format_onchange(buf: &mut [u8], open: bool) -> Result<&str, EncodeError> {
    let payload = if open {
        r#"{"open":true}"#
    } else {
        r#"{"open":false}"#
    };
    write_exact(buf, payload)
}

/// Writes the compact `{"uuid":"<uuid>","result":{"open":<bool>}}` response into `buf`.
///
/// `uuid` is written verbatim as the string token body extracted by
/// [`parse_status_request`], so no re-escaping is performed.
pub fn format_status_response<'a>(
    buf: &'a mut [u8],
    uuid: &str,
    open: bool,
) -> Result<&'a str, EncodeError> {
    let mut pos = 0;
    write(buf, &mut pos, r#"{"uuid":""#)?;
    write(buf, &mut pos, uuid)?;
    write(buf, &mut pos, r#"","result":{"open":"#)?;
    write(buf, &mut pos, if open { "true" } else { "false" })?;
    write(buf, &mut pos, r#"}}"#)?;
    finish(buf, pos)
}

fn is_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn write_exact<'a>(buf: &'a mut [u8], payload: &str) -> Result<&'a str, EncodeError> {
    let bytes = payload.as_bytes();
    if bytes.len() > buf.len() {
        return Err(EncodeError::Truncated);
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    finish(buf, bytes.len())
}

fn write(buf: &mut [u8], pos: &mut usize, payload: &str) -> Result<(), EncodeError> {
    let bytes = payload.as_bytes();
    let end = *pos + bytes.len();
    if end > buf.len() {
        return Err(EncodeError::Truncated);
    }
    buf[*pos..end].copy_from_slice(bytes);
    *pos = end;
    Ok(())
}

fn finish(buf: &[u8], len: usize) -> Result<&str, EncodeError> {
    core::str::from_utf8(&buf[..len]).map_err(|_| EncodeError::Truncated)
}

enum Value<'a> {
    String(&'a str),
    Other,
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.pos += 1;
        Some(byte)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(is_ws) {
            self.advance();
        }
    }

    fn parse_string_raw(&mut self) -> Result<&'a str, StatusRequestError> {
        self.advance();
        let start = self.pos;
        loop {
            match self.next().ok_or(StatusRequestError::Malformed)? {
                b'"' => {
                    let end = self.pos - 1;
                    return core::str::from_utf8(&self.bytes[start..end])
                        .map_err(|_| StatusRequestError::Malformed);
                }
                b'\\' => match self.next().ok_or(StatusRequestError::Malformed)? {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                    b'u' => {
                        for _ in 0..4 {
                            let digit = self.next().ok_or(StatusRequestError::Malformed)?;
                            if !digit.is_ascii_hexdigit() {
                                return Err(StatusRequestError::Malformed);
                            }
                        }
                    }
                    _ => return Err(StatusRequestError::Malformed),
                },
                // Unescaped control characters are invalid in JSON strings and
                // would otherwise be echoed verbatim into the response payload.
                0x00..=0x1f => return Err(StatusRequestError::Malformed),
                _ => {}
            }
        }
    }

    fn parse_value(&mut self) -> Result<Value<'a>, StatusRequestError> {
        self.skip_ws();
        match self.peek() {
            Some(b'"') => Ok(Value::String(self.parse_string_raw()?)),
            Some(b'{') => {
                self.skip_object()?;
                Ok(Value::Other)
            }
            Some(b'[') => {
                self.skip_array()?;
                Ok(Value::Other)
            }
            Some(b't') | Some(b'f') | Some(b'n') => {
                self.skip_literal()?;
                Ok(Value::Other)
            }
            Some(b'-') | Some(b'0'..=b'9') => {
                self.skip_number()?;
                Ok(Value::Other)
            }
            _ => Err(StatusRequestError::Malformed),
        }
    }

    fn skip_value_only(&mut self) -> Result<(), StatusRequestError> {
        match self.parse_value()? {
            Value::String(_) | Value::Other => Ok(()),
        }
    }

    fn skip_object(&mut self) -> Result<(), StatusRequestError> {
        self.advance();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.advance();
            return Ok(());
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(StatusRequestError::Malformed);
            }
            self.parse_string_raw()?;

            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(StatusRequestError::Malformed);
            }
            self.advance();

            self.skip_value_only()?;

            self.skip_ws();
            match self.peek() {
                Some(b',') => self.advance(),
                Some(b'}') => {
                    self.advance();
                    return Ok(());
                }
                _ => return Err(StatusRequestError::Malformed),
            }
        }
    }

    fn skip_array(&mut self) -> Result<(), StatusRequestError> {
        self.advance();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.advance();
            return Ok(());
        }
        loop {
            self.skip_value_only()?;

            self.skip_ws();
            match self.peek() {
                Some(b',') => self.advance(),
                Some(b']') => {
                    self.advance();
                    return Ok(());
                }
                _ => return Err(StatusRequestError::Malformed),
            }
        }
    }

    fn skip_literal(&mut self) -> Result<(), StatusRequestError> {
        for literal in ["true", "false", "null"] {
            if self.bytes[self.pos..].starts_with(literal.as_bytes()) {
                self.pos += literal.len();
                return Ok(());
            }
        }
        Err(StatusRequestError::Malformed)
    }

    fn skip_number(&mut self) -> Result<(), StatusRequestError> {
        if self.peek() == Some(b'-') {
            self.advance();
        }
        if !self.skip_digits() {
            return Err(StatusRequestError::Malformed);
        }
        if self.peek() == Some(b'.') {
            self.advance();
            if !self.skip_digits() {
                return Err(StatusRequestError::Malformed);
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.advance();
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.advance();
            }
            if !self.skip_digits() {
                return Err(StatusRequestError::Malformed);
            }
        }
        Ok(())
    }

    fn skip_digits(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.advance();
        }
        self.pos > start
    }
}

#[cfg(test)]
mod tests;
