//! OTA task: resolve, fetch, verify and (only then) commit an update.
//!
//! The task runs one check at boot (once DHCP is up) and then one per
//! [`crate::telemetry::RESET_REQUEST_SIGNAL`] notification; there is no
//! periodic poll. A failed check is logged and signals the onboard beacon; the
//! task never resets on failure. The control path is never blocked: every
//! network wait is a timeout-wrapped await.

use core::fmt::Write as _;

use garagedoor_core::ota::{
    apply_update, decide, parse_sha256_hex, parse_url, parse_version, Decision, UpdateError,
};

use crate::radio::NetStack;
use crate::{beacon, secrets, update};

use super::transport::{
    build_request, open, resolve, HttpBody, READ_CHUNK, RECORD_BYTES, TCP_BYTES,
};
use super::{FetchError, OtaError, TextBuf};

const VERSION_MAX_BYTES: usize = 64;
const SHA_MAX_BYTES: usize = 128;
const NAME_MAX_BYTES: usize = 48;

/// One boot check after DHCP, then one check per reset request. The task owns
/// `updater` for the life of the program and only touches flash after a
/// response head and a hash have been accepted. A failed check blinks its
/// stage code on the onboard LED so a human can report it without a probe.
#[embassy_executor::task]
pub(super) async fn ota_task(stack: NetStack, mut updater: update::Updater) -> ! {
    run_check(stack, &mut updater).await;
    loop {
        crate::telemetry::RESET_REQUEST_SIGNAL.wait().await;
        run_check(stack, &mut updater).await;
    }
}

/// Run one check, logging and blinking on failure. Never resets.
async fn run_check(stack: NetStack, updater: &mut update::Updater) {
    if let Err(error) = check_and_update(stack, updater).await {
        defmt::warn!("ota_failed code={}", error.code());
        beacon::BEACON_SIGNAL.signal(beacon::BeaconState::Error(error.blink_code()));
    }
}

/// Resolve, fetch, verify and (only then) mark the image. Returns `Ok(())` for
/// every transient condition (404, no update) as well as for errors that do not
/// warrant a reset; the caller logs the error code. Never loops, never resets.
async fn check_and_update(stack: NetStack, updater: &mut update::Updater) -> Result<(), OtaError> {
    stack.wait_config_up().await;

    let uri = parse_url(secrets::OTA_URL, secrets::OTA_PROJECT).map_err(|_| {
        defmt::warn!("ota_url_invalid");
        OtaError::Url
    })?;
    let endpoint = resolve(stack, &uri).await?;

    let mut tcp_rx = [0u8; TCP_BYTES];
    let mut tcp_tx = [0u8; TCP_BYTES];
    let mut rec_read = [0u8; RECORD_BYTES];
    let mut rec_write = [0u8; RECORD_BYTES];
    let mut leftover = [0u8; READ_CHUNK];

    let remote = {
        let request = build_request(&uri, "version")?;
        let mut link = open(
            stack,
            endpoint,
            &uri,
            &mut tcp_rx,
            &mut tcp_tx,
            &mut rec_read,
            &mut rec_write,
        )
        .await?;
        let (head, extra) = link
            .send(request.as_bytes(), &mut leftover)
            .await
            .map_err(fetch_failed)?;
        if head.status != 200 {
            defmt::info!("ota_version_missing status={}", head.status);
            return Ok(());
        }
        let len = head.content_length.ok_or(OtaError::Head)?;
        if len > VERSION_MAX_BYTES as u64 {
            defmt::warn!("ota_version_too_long");
            return Err(OtaError::Head);
        }
        let mut body = [0u8; VERSION_MAX_BYTES];
        let n = link
            .read_small(&leftover[..extra], len, &mut body)
            .await
            .map_err(fetch_failed)?;
        parse_version(&body[..n]).ok_or_else(|| {
            defmt::warn!("ota_version_invalid");
            OtaError::Version
        })?
    };

    match decide(crate::VERSION, remote) {
        Decision::Skip => {
            defmt::info!("ota_check local={} remote={}", crate::VERSION, remote);
            defmt::info!("ota_no_update");
            return Ok(());
        }
        Decision::Update => defmt::info!("ota_check local={} remote={}", crate::VERSION, remote),
    }

    let expected = {
        let mut name = TextBuf::<NAME_MAX_BYTES>::new();
        write!(name, "{}.bin.sha256", remote).map_err(|_| OtaError::Request)?;
        let request = build_request(&uri, name.as_str())?;
        let mut link = open(
            stack,
            endpoint,
            &uri,
            &mut tcp_rx,
            &mut tcp_tx,
            &mut rec_read,
            &mut rec_write,
        )
        .await?;
        let (head, extra) = link
            .send(request.as_bytes(), &mut leftover)
            .await
            .map_err(fetch_failed)?;
        if head.status != 200 {
            defmt::info!("ota_sha_missing status={}", head.status);
            return Ok(());
        }
        let len = head.content_length.ok_or(OtaError::Head)?;
        if len > SHA_MAX_BYTES as u64 {
            defmt::warn!("ota_sha_too_long");
            return Err(OtaError::Head);
        }
        let mut body = [0u8; SHA_MAX_BYTES];
        let n = link
            .read_small(&leftover[..extra], len, &mut body)
            .await
            .map_err(fetch_failed)?;
        parse_sha256_hex(&body[..n]).ok_or_else(|| {
            defmt::warn!("ota_sha_invalid");
            OtaError::Sha
        })?
    };

    let mut name = TextBuf::<NAME_MAX_BYTES>::new();
    write!(name, "{}.bin", remote).map_err(|_| OtaError::Request)?;
    let request = build_request(&uri, name.as_str())?;
    let mut link = open(
        stack,
        endpoint,
        &uri,
        &mut tcp_rx,
        &mut tcp_tx,
        &mut rec_read,
        &mut rec_write,
    )
    .await?;
    let (head, extra) = link
        .send(request.as_bytes(), &mut leftover)
        .await
        .map_err(fetch_failed)?;
    if head.status != 200 {
        defmt::info!("ota_bin_missing status={}", head.status);
        return Ok(());
    }
    let len = head.content_length.ok_or(OtaError::Head)?;
    defmt::info!("ota_head status={} len={}", head.status, len);

    let result = {
        let mut body = HttpBody {
            link: &mut link,
            leftover: &leftover[..extra],
        };
        defmt::info!("ota_streaming len={}", len);
        apply_update(updater, &mut body, len, &expected).await
    };
    result.map_err(|error| {
        let code = update_code(&error);
        if matches!(error, UpdateError::HashMismatch) {
            defmt::warn!("ota_hash_mismatch");
        }
        OtaError::Update(code)
    })?;

    defmt::info!("ota_hash_ok");
    defmt::info!("ota_marked");
    defmt::info!("ota_flash_window_max_us={}", update::max_window_us());
    cortex_m::peripheral::SCB::sys_reset();
}

fn fetch_failed(error: FetchError) -> OtaError {
    defmt::warn!("ota_fetch_failed code={}", error.code());
    OtaError::Transport(error)
}

fn update_code<FE, RE>(error: &UpdateError<FE, RE>) -> &'static str {
    match error {
        UpdateError::Oversize { .. } => "oversize",
        UpdateError::TooShort => "too_short",
        UpdateError::TooLong => "too_long",
        UpdateError::HashMismatch => "hash_mismatch",
        UpdateError::Flash(_) => "flash",
        UpdateError::Read(_) => "read",
    }
}
