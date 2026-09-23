use super::*;

#[test]
fn parse_minimal_uuid() {
    assert_eq!(parse_status_request(br#"{"uuid":"123"}"#), Ok("123"));
}

#[test]
fn parse_full_uuid() {
    assert_eq!(
        parse_status_request(br#"{"uuid":"550e8400-e29b-41d4-a716-446655440000"}"#),
        Ok("550e8400-e29b-41d4-a716-446655440000")
    );
}

#[test]
fn parse_tolerates_whitespace() {
    assert_eq!(parse_status_request(b"{ \"uuid\" : \"abc\" }"), Ok("abc"));
}

#[test]
fn parse_accepts_reordered_and_extra_keys() {
    assert_eq!(
        parse_status_request(br#"{"foo":1,"uuid":"abc","bar":true}"#),
        Ok("abc")
    );
}

#[test]
fn parse_ignores_nested_uuid_key() {
    assert_eq!(
        parse_status_request(br#"{"a":{"uuid":"x"},"uuid":"y"}"#),
        Ok("y")
    );
}

#[test]
fn parse_skips_arrays_before_uuid() {
    assert_eq!(
        parse_status_request(br#"{"a":[1,2,{"uuid":"x"}],"uuid":"z"}"#),
        Ok("z")
    );
}

#[test]
fn parse_first_uuid_wins() {
    assert_eq!(
        parse_status_request(br#"{"uuid":"first","uuid":"second"}"#),
        Ok("first")
    );
}

#[test]
fn parse_honours_escaped_quote() {
    assert_eq!(parse_status_request(br#"{"uuid":"a\"b"}"#), Ok("a\\\"b"));
}

#[test]
fn parse_honours_escaped_backslash() {
    assert_eq!(parse_status_request(br#"{"uuid":"a\\b"}"#), Ok("a\\\\b"));
}

#[test]
fn parse_rejects_non_object_literal() {
    assert_eq!(
        parse_status_request(b"not json"),
        Err(StatusRequestError::NotObject)
    );
}

#[test]
fn parse_rejects_top_level_array() {
    assert_eq!(
        parse_status_request(b"[]"),
        Err(StatusRequestError::NotObject)
    );
}

#[test]
fn parse_rejects_empty_object() {
    assert_eq!(
        parse_status_request(b"{}"),
        Err(StatusRequestError::MissingUuid)
    );
}

#[test]
fn parse_rejects_non_string_uuid() {
    assert_eq!(
        parse_status_request(br#"{"uuid":123}"#),
        Err(StatusRequestError::UuidNotString)
    );
    assert_eq!(
        parse_status_request(br#"{"uuid":null}"#),
        Err(StatusRequestError::UuidNotString)
    );
}

#[test]
fn parse_rejects_missing_value() {
    assert_eq!(
        parse_status_request(br#"{"uuid":}"#),
        Err(StatusRequestError::Malformed)
    );
}

#[test]
fn parse_rejects_unterminated_string() {
    assert_eq!(
        parse_status_request(br#"{"uuid":"abc"#),
        Err(StatusRequestError::Malformed)
    );
}

#[test]
fn parse_rejects_empty_uuid() {
    assert_eq!(
        parse_status_request(br#"{"uuid":""}"#),
        Err(StatusRequestError::EmptyUuid)
    );
}

#[test]
fn parse_rejects_trailing_garbage() {
    assert_eq!(
        parse_status_request(br#"{"uuid":"abc"} x"#),
        Err(StatusRequestError::Malformed)
    );
}

#[test]
fn parse_rejects_unterminated_object() {
    assert_eq!(
        parse_status_request(br#"{"uuid":"abc""#),
        Err(StatusRequestError::Malformed)
    );
}

#[test]
fn parse_rejects_only_nested_uuid() {
    assert_eq!(
        parse_status_request(br#"{"a":{"uuid":"x"}}"#),
        Err(StatusRequestError::MissingUuid)
    );
}

#[test]
fn parse_rejects_invalid_escape() {
    assert_eq!(
        parse_status_request(br#"{"uuid":"a\qb"}"#),
        Err(StatusRequestError::Malformed)
    );
}

#[test]
fn parse_rejects_raw_control_char() {
    assert_eq!(
        parse_status_request(b"{\"uuid\":\"a\nb\"}"),
        Err(StatusRequestError::Malformed)
    );
}

#[test]
fn parse_rejects_incomplete_unicode_escape() {
    assert_eq!(
        parse_status_request(br#"{"uuid":"a\u12"}"#),
        Err(StatusRequestError::Malformed)
    );
    assert_eq!(
        parse_status_request(br#"{"uuid":"a\u12g4"}"#),
        Err(StatusRequestError::Malformed)
    );
}

#[test]
fn format_onchange_false_exact_bytes() {
    let mut buf = [0u8; 32];
    assert_eq!(format_onchange(&mut buf, false), Ok(r#"{"open":false}"#));
}

#[test]
fn format_onchange_true_exact_bytes() {
    let mut buf = [0u8; 32];
    assert_eq!(format_onchange(&mut buf, true), Ok(r#"{"open":true}"#));
}

#[test]
fn format_status_response_true_exact_bytes() {
    let mut buf = [0u8; 64];
    assert_eq!(
        format_status_response(&mut buf, "123", true),
        Ok(r#"{"uuid":"123","result":{"open":true}}"#)
    );
}

#[test]
fn format_status_response_false_exact_bytes() {
    let mut buf = [0u8; 64];
    assert_eq!(
        format_status_response(&mut buf, "123", false),
        Ok(r#"{"uuid":"123","result":{"open":false}}"#)
    );
}

#[test]
fn format_onchange_truncated_without_panic() {
    let mut buf = [0u8; 4];
    assert_eq!(
        format_onchange(&mut buf, false),
        Err(EncodeError::Truncated)
    );
}

#[test]
fn format_status_response_truncated_without_panic() {
    let mut buf = [0u8; 8];
    assert_eq!(
        format_status_response(&mut buf, "123", true),
        Err(EncodeError::Truncated)
    );
}

#[test]
fn format_fits_exact_buffer() {
    let mut onchange = [0u8; 13];
    assert_eq!(format_onchange(&mut onchange, true), Ok(r#"{"open":true}"#));

    let mut status = [0u8; 37];
    assert_eq!(
        format_status_response(&mut status, "123", true),
        Ok(r#"{"uuid":"123","result":{"open":true}}"#)
    );
}
