use super::*;

use std::vec::Vec;

const HEX_ABC: &[u8; 64] = b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const HEX_PATTERN_5000: &[u8; 64] =
    b"69dbee893909fa17d1be397e0c07691336fe42049c29d403467d3d4a1fc3b5a1";

fn hex32(hex: &[u8; 64]) -> [u8; 32] {
    parse_sha256_hex(hex).expect("test vector must be valid lowercase hex")
}

fn parse_head(bytes: &[u8]) -> (HeadEvent, usize) {
    let mut parser = HeadParser::new();
    let mut last = HeadEvent::NeedMore;
    for &b in bytes {
        last = parser.push(b);
    }
    (last, parser.consumed())
}

// --- decide ---------------------------------------------------------------

#[test]
fn decide_table() {
    assert_eq!(decide(1, 2), Decision::Update);
    assert_eq!(decide(2, 2), Decision::Skip);
    assert_eq!(decide(5, 3), Decision::Update);
    assert_eq!(decide(0, 0), Decision::Skip);
    assert_eq!(decide(u32::MAX, 0), Decision::Update);
}

// --- HeadParser -----------------------------------------------------------

#[test]
fn head_completes_one_byte_at_a_time() {
    let head = b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\n\r\n";
    let (event, consumed) = parse_head(head);
    assert_eq!(
        event,
        HeadEvent::Complete(ResponseHead {
            status: 200,
            content_length: Some(5)
        })
    );
    assert_eq!(consumed, head.len());
}

#[test]
fn head_completes_at_every_split_boundary() {
    let head = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n";
    let expected = HeadEvent::Complete(ResponseHead {
        status: 200,
        content_length: Some(5),
    });
    for split in 0..=head.len() {
        let mut parser = HeadParser::new();
        let mut last = HeadEvent::NeedMore;
        for &b in &head[..split] {
            last = parser.push(b);
        }
        for &b in &head[split..] {
            last = parser.push(b);
        }
        assert_eq!(last, expected, "split at {split}");
        assert_eq!(parser.consumed(), head.len(), "consumed at {split}");
    }
}

#[test]
fn head_accepts_404_without_content_length() {
    let head = b"HTTP/1.0 404 Not Found\r\n\r\n";
    let (event, consumed) = parse_head(head);
    assert_eq!(
        event,
        HeadEvent::Complete(ResponseHead {
            status: 404,
            content_length: None
        })
    );
    assert_eq!(consumed, head.len());
}

#[test]
fn head_accepts_404_with_content_length() {
    let head = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
    let (event, consumed) = parse_head(head);
    assert_eq!(
        event,
        HeadEvent::Complete(ResponseHead {
            status: 404,
            content_length: Some(0)
        })
    );
    assert_eq!(consumed, head.len());
}

#[test]
fn head_ignores_body_bytes_after_terminator() {
    let head = b"HTTP/1.0 200 OK\r\nContent-Length: 3\r\n\r\n";
    let mut parser = HeadParser::new();
    let mut last = HeadEvent::NeedMore;
    for &b in head {
        last = parser.push(b);
    }
    assert_eq!(
        last,
        HeadEvent::Complete(ResponseHead {
            status: 200,
            content_length: Some(3)
        })
    );
    let consumed = parser.consumed();
    assert_eq!(consumed, head.len());
    assert_eq!(
        parser.push(b'x'),
        HeadEvent::Complete(ResponseHead {
            status: 200,
            content_length: Some(3)
        })
    );
    assert_eq!(parser.consumed(), consumed);
}

#[test]
fn head_rejects_transfer_encoding() {
    let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 5\r\n\r\n";
    let (event, _) = parse_head(head);
    assert_eq!(event, HeadEvent::Reject(HeadError::TransferEncoding));
}

#[test]
fn head_rejects_missing_content_length() {
    let head = b"HTTP/1.1 200 OK\r\nHost: example.com\r\n\r\n";
    let (event, _) = parse_head(head);
    assert_eq!(event, HeadEvent::Reject(HeadError::MissingContentLength));
}

#[test]
fn head_rejects_duplicate_content_length() {
    let head = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n";
    let (event, _) = parse_head(head);
    assert_eq!(event, HeadEvent::Reject(HeadError::DuplicateContentLength));
}

#[test]
fn head_rejects_malformed_status_line() {
    let (event, _) = parse_head(b"NOTHTTP 200 OK\r\n\r\n");
    assert_eq!(event, HeadEvent::Reject(HeadError::MalformedStatusLine));
}

#[test]
fn head_rejects_too_few_status_tokens() {
    let (event, _) = parse_head(b"HTTP/1.0 200\r\n\r\n");
    assert_eq!(event, HeadEvent::Reject(HeadError::TooFewStatusTokens));
}

#[test]
fn head_rejects_non_numeric_status() {
    let (event, _) = parse_head(b"HTTP/1.0 abc OK\r\n\r\n");
    assert_eq!(event, HeadEvent::Reject(HeadError::NonNumericStatus));
}

#[test]
fn head_rejects_unexpected_status() {
    let (event, _) = parse_head(b"HTTP/1.1 500 Server Error\r\nContent-Length: 1\r\n\r\n");
    assert_eq!(event, HeadEvent::Reject(HeadError::UnexpectedStatus));
}

#[test]
fn head_rejects_malformed_header() {
    let (event, _) = parse_head(b"HTTP/1.0 200 OK\r\nno-colon-here\r\n\r\n");
    assert_eq!(event, HeadEvent::Reject(HeadError::MalformedHeader));
}

#[test]
fn head_rejects_oversized_header_block() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"HTTP/1.0 200 OK\r\n");
    bytes.extend(core::iter::repeat_n(b'a', 2100));
    let (event, _) = parse_head(&bytes);
    assert_eq!(event, HeadEvent::Reject(HeadError::OversizedHeader));
}

// --- parse_version --------------------------------------------------------

#[test]
fn parse_version_valid() {
    assert_eq!(parse_version(b"15"), Some(15));
    assert_eq!(parse_version(b"0"), Some(0));
    assert_eq!(parse_version(b"4294967295"), Some(u32::MAX));
}

#[test]
fn parse_version_tolerates_whitespace() {
    assert_eq!(parse_version(b"  42\n"), Some(42));
    assert_eq!(parse_version(b"\t7\r\n"), Some(7));
}

#[test]
fn parse_version_rejects() {
    assert_eq!(parse_version(b""), None);
    assert_eq!(parse_version(b"   "), None);
    assert_eq!(parse_version(b"12a"), None);
    assert_eq!(parse_version(b"-1"), None);
    assert_eq!(parse_version(b"4294967296"), None);
    assert_eq!(parse_version(b"99999999999999999999"), None);
}

// --- parse_sha256_hex -----------------------------------------------------

#[test]
fn parse_sha256_hex_valid_and_whitespace() {
    let expected = hex32(HEX_ABC);
    assert_eq!(parse_sha256_hex(HEX_ABC), Some(expected));
    let mut with_ws = Vec::new();
    with_ws.extend_from_slice(b"  ");
    with_ws.extend_from_slice(HEX_ABC);
    with_ws.extend_from_slice(b"\r\n");
    assert_eq!(parse_sha256_hex(&with_ws), Some(expected));
}

#[test]
fn parse_sha256_hex_rejects_uppercase() {
    let upper = b"BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD";
    assert_eq!(parse_sha256_hex(upper), None);
}

#[test]
fn parse_sha256_hex_rejects_wrong_length() {
    assert_eq!(parse_sha256_hex(&HEX_ABC[..63]), None);
    let mut long = HEX_ABC.to_vec();
    long.push(b'0');
    assert_eq!(parse_sha256_hex(&long), None);
    assert_eq!(parse_sha256_hex(b""), None);
}

#[test]
fn parse_sha256_hex_rejects_non_hex() {
    let mut bad = *HEX_ABC;
    bad[0] = b'z';
    assert_eq!(parse_sha256_hex(&bad), None);
}

// --- parse_url ------------------------------------------------------------

#[test]
fn parse_url_https_default_port() {
    let uri = parse_url("https://example.com", "proj").unwrap();
    assert!(uri.tls);
    assert_eq!(uri.host, "example.com");
    assert_eq!(uri.port, 443);
    assert_eq!(uri.path(), "/proj");
}

#[test]
fn parse_url_https_port_and_prefix() {
    let uri = parse_url("https://example.com:8443/pfx", "proj").unwrap();
    assert!(uri.tls);
    assert_eq!(uri.host, "example.com");
    assert_eq!(uri.port, 8443);
    assert_eq!(uri.path(), "/pfx/proj");
}

#[test]
fn parse_url_multi_segment_prefix_with_trailing_slash() {
    let uri = parse_url("https://h/a/b/", "proj").unwrap();
    assert_eq!(uri.path(), "/a/b/proj");
}

#[test]
fn parse_url_http_default_port() {
    let uri = parse_url("http://example.com", "proj").unwrap();
    assert!(!uri.tls);
    assert_eq!(uri.port, 80);
    assert_eq!(uri.path(), "/proj");
}

#[test]
fn parse_url_literal_ipv4() {
    let uri = parse_url("https://192.168.1.10:9000", "proj").unwrap();
    assert!(uri.tls);
    assert_eq!(uri.host, "192.168.1.10");
    assert_eq!(uri.port, 9000);
}

#[test]
fn parse_url_rejects_bad_inputs() {
    assert_eq!(
        parse_url("example.com", "proj").unwrap_err(),
        UrlError::Scheme
    );
    assert_eq!(parse_url("ftp://h", "proj").unwrap_err(), UrlError::Scheme);
    assert_eq!(parse_url("https://", "proj").unwrap_err(), UrlError::Host);
    assert_eq!(
        parse_url("https://bad host", "proj").unwrap_err(),
        UrlError::Host
    );
    assert_eq!(
        parse_url("https://h:0", "proj").unwrap_err(),
        UrlError::Port
    );
    assert_eq!(
        parse_url("https://h:abc", "proj").unwrap_err(),
        UrlError::Port
    );
    assert_eq!(
        parse_url("https://h", "bad proj").unwrap_err(),
        UrlError::Project
    );
    assert_eq!(parse_url("https://h", "").unwrap_err(), UrlError::Project);
    assert_eq!(
        parse_url("https://h", "p!x").unwrap_err(),
        UrlError::Project
    );
}

// --- base64 ---------------------------------------------------------------

fn assert_b64(input: &[u8], expected: &str) {
    let mut out = [0u8; 32];
    let n = base64(input, &mut out).expect("buffer large enough");
    assert_eq!(&out[..n], expected.as_bytes());
}

#[test]
fn base64_rfc4648_vectors() {
    assert_b64(b"", "");
    assert_b64(b"f", "Zg==");
    assert_b64(b"fo", "Zm8=");
    assert_b64(b"foo", "Zm9v");
    assert_b64(b"foob", "Zm9vYg==");
    assert_b64(b"fooba", "Zm9vYmE=");
    assert_b64(b"foobar", "Zm9vYmFy");
}

#[test]
fn base64_rejects_small_buffer() {
    let mut out = [0u8; 4];
    assert_eq!(base64(b"foobar", &mut out), None);
}

// --- basic_authorization --------------------------------------------------

fn assert_basic(user: &str, pass: &str, expected: &str) {
    let mut out = [0u8; 64];
    let n = basic_authorization(user, pass, &mut out).expect("buffer large enough");
    assert_eq!(&out[..n], expected.as_bytes());
}

#[test]
fn basic_authorization_rfc7617_example() {
    assert_basic(
        "Aladdin",
        "open sesame",
        "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==",
    );
}

#[test]
fn basic_authorization_single_side() {
    assert_basic("user", "", "Basic dXNlcjo=");
    assert_basic("", "pass", "Basic OnBhc3M=");
}

#[test]
fn basic_authorization_empty_credentials_is_none() {
    let mut out = [0u8; 64];
    assert_eq!(basic_authorization("", "", &mut out), None);
}

#[test]
fn basic_authorization_rejects_small_buffer() {
    let mut out = [0u8; 6];
    assert_eq!(basic_authorization("user", "pass", &mut out), None);
}

// --- apply_update ---------------------------------------------------------

#[derive(Default)]
struct MockFlasher {
    writes: Vec<(usize, Vec<u8>)>,
    marks: usize,
    fail_write: bool,
    fail_mark: bool,
}

impl Flasher for MockFlasher {
    type Error = ();

    async fn write(&mut self, offset: usize, data: &[u8]) -> Result<(), ()> {
        if self.fail_write {
            return Err(());
        }
        self.writes.push((offset, data.to_vec()));
        Ok(())
    }

    async fn mark_updated(&mut self) -> Result<(), ()> {
        if self.fail_mark {
            return Err(());
        }
        self.marks += 1;
        Ok(())
    }
}

struct MockReader {
    data: Vec<u8>,
    pos: usize,
    lie: Option<usize>,
    fail: bool,
}

impl MockReader {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            pos: 0,
            lie: None,
            fail: false,
        }
    }
}

impl BodyReader for MockReader {
    type Error = ();

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        if self.fail {
            return Err(());
        }
        if let Some(n) = self.lie.take() {
            return Ok(n);
        }
        let remaining = self.data.len() - self.pos;
        let n = remaining.min(buf.len());
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[test]
fn apply_update_rejects_oversize_before_any_flash_op() {
    let capacity = crate::flash::ACTIVE_BYTES;
    let over = capacity as u64 + 1;
    let mut flasher = MockFlasher::default();
    let mut reader = MockReader::new([0u8; 16].to_vec());
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, over, &[0u8; 32]));
    assert_eq!(
        result,
        Err(UpdateError::Oversize {
            length: over,
            capacity
        })
    );
    assert!(flasher.writes.is_empty());
    assert_eq!(flasher.marks, 0);
}

#[test]
fn apply_update_accepts_exact_capacity_bound() {
    let capacity = crate::flash::ACTIVE_BYTES as u64;
    let mut flasher = MockFlasher::default();
    let mut reader = MockReader::new(Vec::new());
    let result = pollster::block_on(apply_update(
        &mut flasher,
        &mut reader,
        capacity,
        &[0u8; 32],
    ));
    assert_eq!(result, Err(UpdateError::TooShort));
    assert_eq!(flasher.marks, 0);
}

#[test]
fn apply_update_happy_path_known_vector() {
    let expected = hex32(HEX_ABC);
    let mut flasher = MockFlasher::default();
    let mut reader = MockReader::new(b"abc".to_vec());
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, 3, &expected));
    assert_eq!(result, Ok(()));
    assert_eq!(flasher.writes.len(), 1);
    assert_eq!(flasher.writes[0].0, 0);
    assert_eq!(flasher.writes[0].1, b"abc");
    assert_eq!(flasher.marks, 1);
}

#[test]
fn apply_update_multi_chunk_offsets_and_digest() {
    let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    let expected = hex32(HEX_PATTERN_5000);
    let mut flasher = MockFlasher::default();
    let mut reader = MockReader::new(data);
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, 5000, &expected));
    assert_eq!(result, Ok(()));
    assert_eq!(flasher.writes.len(), 2);
    assert_eq!(flasher.writes[0].0, 0);
    assert_eq!(flasher.writes[0].1.len(), CHUNK_BYTES);
    assert_eq!(flasher.writes[1].0, CHUNK_BYTES);
    assert_eq!(flasher.writes[1].1.len(), 5000 - CHUNK_BYTES);
    assert_eq!(flasher.marks, 1);
}

#[test]
fn apply_update_bit_flip_is_hash_mismatch_without_mark() {
    let expected = hex32(HEX_ABC);
    let mut data = b"abc".to_vec();
    data[1] ^= 0x01;
    let mut flasher = MockFlasher::default();
    let mut reader = MockReader::new(data);
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, 3, &expected));
    assert_eq!(result, Err(UpdateError::HashMismatch));
    assert_eq!(flasher.writes.len(), 1);
    assert_eq!(flasher.marks, 0);
}

#[test]
fn apply_update_truncated_is_too_short() {
    let mut flasher = MockFlasher::default();
    let mut reader = MockReader::new(b"abc".to_vec());
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, 10, &[0u8; 32]));
    assert_eq!(result, Err(UpdateError::TooShort));
    assert_eq!(flasher.marks, 0);
}

#[test]
fn apply_update_over_long_is_too_long() {
    let mut flasher = MockFlasher::default();
    let mut reader = MockReader {
        data: [0u8; 100].to_vec(),
        pos: 0,
        lie: Some(200),
        fail: false,
    };
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, 100, &[0u8; 32]));
    assert_eq!(result, Err(UpdateError::TooLong));
    assert_eq!(flasher.marks, 0);
}

#[test]
fn apply_update_maps_read_error() {
    let mut flasher = MockFlasher::default();
    let mut reader = MockReader {
        data: Vec::new(),
        pos: 0,
        lie: None,
        fail: true,
    };
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, 10, &[0u8; 32]));
    assert_eq!(result, Err(UpdateError::Read(())));
    assert_eq!(flasher.marks, 0);
}

#[test]
fn apply_update_maps_flash_error() {
    let expected = hex32(HEX_ABC);
    let mut flasher = MockFlasher {
        fail_write: true,
        ..MockFlasher::default()
    };
    let mut reader = MockReader::new(b"abc".to_vec());
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, 3, &expected));
    assert_eq!(result, Err(UpdateError::Flash(())));
    assert_eq!(flasher.marks, 0);
}

#[test]
fn apply_update_maps_mark_error() {
    let expected = hex32(HEX_ABC);
    let mut flasher = MockFlasher {
        fail_mark: true,
        ..MockFlasher::default()
    };
    let mut reader = MockReader::new(b"abc".to_vec());
    let result = pollster::block_on(apply_update(&mut flasher, &mut reader, 3, &expected));
    assert_eq!(result, Err(UpdateError::Flash(())));
    assert_eq!(flasher.writes.len(), 1);
    assert_eq!(flasher.marks, 0);
}
