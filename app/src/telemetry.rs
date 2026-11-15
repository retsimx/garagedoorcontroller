//! MQTT telemetry and control task.
//!
//! Owns one long-lived `minimq` session over the `embassy-net` stack: it
//! reconnects with exponential backoff across network loss, subscribes to the
//! command topics (`trigger`/`status`/`reset`), publishes state changes on
//! `onchange`, and answers correlated status requests. Network, parse, and
//! publish failures are logged and recovered from — this task never panics the
//! MCU, and only resets after [`NO_IP_REBOOT_MS`] without a healthy session so
//! offline door control stays functional across transient outages.

use embassy_futures::select::{select, Either};
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Instant, Timer};
use garagedoor_core::mqtt;
use garagedoor_core::wifi::{
    mqtt_session_healthy, NoIpWatchdog, RejoinCounter, DHCP_TIMEOUT_MS, MQTT_IO_TIMEOUT_MS,
    MQTT_POLL_TIMEOUT_MS, MQTT_TCP_KEEPALIVE_MS, MQTT_TCP_TIMEOUT_MS, NO_IP_REBOOT_MS,
};
use garagedoor_core::{DoorCommand, DoorState};
use minimq::{Buffers, ConfigBuilder, Connection, Publication, QoS, Session, TopicFilter};

use crate::actuator::DOOR_CMD_CHANNEL;
use crate::radio::NetStack;
use crate::secrets;
use crate::sensor::DOOR_STATE_SIGNAL;
use crate::wifi::WIFI_REJOIN_SIGNAL;

/// Integration point for the GDC-7 (issue #8) OTA worker.
///
/// Signalled when `garagedoor/reset` is received; the worker awaits this signal
/// to begin an update check. No OTA engine exists here, by design.
pub static RESET_REQUEST_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Client identifier and keepalive advertised in `CONNECT`.
const CLIENT_ID: &str = "garagedoorcontroller";
const KEEPALIVE_SECS: u16 = 60;

/// Broker port used when `secrets::MQTT_BROKER` omits one.
const DEFAULT_BROKER_PORT: u16 = 1883;

/// Bounds the initial TCP handshake; once connected, the socket switches to
/// [`MQTT_TCP_KEEPALIVE_MS`] and [`MQTT_TCP_TIMEOUT_MS`] so half-open idle TCP
/// connections and blackholed MQTT packets are detected at the transport layer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// minimq packet buffers (design table): largest inbound is a status request,
/// largest outbound is a status response, both a fraction of these sizes.
const MQTT_RX_LEN: usize = 256;
const MQTT_TX_LEN: usize = 256;
/// TCP segment buffers carrying the MQTT stream.
const TCP_RX_LEN: usize = 1024;
const TCP_TX_LEN: usize = 1024;

/// Fixed-capacity copy of a status-request correlation UUID.
const UUID_MAX: usize = 64;
/// Slack for the largest outbound JSON payload (a status response).
const PAYLOAD_MAX: usize = 160;

/// Owned classification of one inbound event, decoupled from the borrowed
/// `InboundPublish` so the connection can be published to afterwards.
enum Action {
    Trigger,
    Reset,
    Status {
        uuid: [u8; UUID_MAX],
        uuid_len: usize,
    },
    StateChange(DoorState),
    Disconnected,
    Unknown,
}

fn is_open(state: DoorState) -> bool {
    state == DoorState::Open
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Split `mqtt://<host>[:<port>]` into host and port, defaulting the port.
fn parse_broker(broker: &str) -> (&str, u16) {
    let rest = broker.strip_prefix("mqtt://").unwrap_or(broker);
    match rest.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().unwrap_or(DEFAULT_BROKER_PORT)),
        None => (rest, DEFAULT_BROKER_PORT),
    }
}

/// Classify one inbound publish, copying any correlation UUID into stack storage
/// so no borrow of the inbound packet escapes.
fn classify(topic: &str, payload: &[u8]) -> Action {
    if topic == mqtt::TOPIC_TRIGGER {
        Action::Trigger
    } else if topic == mqtt::TOPIC_RESET {
        Action::Reset
    } else if topic == mqtt::TOPIC_STATUS {
        match mqtt::parse_status_request(payload) {
            Ok(uuid) => {
                let bytes = uuid.as_bytes();
                if bytes.len() > UUID_MAX {
                    defmt::warn!("mqtt: status uuid too long; discarding request");
                    Action::Unknown
                } else {
                    let mut uuid = [0u8; UUID_MAX];
                    uuid[..bytes.len()].copy_from_slice(bytes);
                    Action::Status {
                        uuid,
                        uuid_len: bytes.len(),
                    }
                }
            }
            Err(_) => {
                defmt::warn!("mqtt: malformed status request; discarding");
                Action::Unknown
            }
        }
    } else {
        defmt::warn!("mqtt: ignoring unknown topic '{}'", topic);
        Action::Unknown
    }
}

/// Publish `payload` to `topic` with a bounded timeout, returning `false` if the
/// transport publish fails or times out.
async fn publish_payload<'buf, IO: minimq::Io>(
    conn: &mut Connection<'_, 'buf, IO>,
    topic: &str,
    payload: &str,
) -> bool {
    let publication = Publication::bytes(topic, payload.as_bytes()).qos(QoS::AtMostOnce);
    match with_timeout(
        Duration::from_millis(MQTT_IO_TIMEOUT_MS),
        conn.publish(publication),
    )
    .await
    {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            defmt::warn!(
                "mqtt: publish to '{}' failed: {:?}",
                topic,
                defmt::Debug2Format(&err)
            );
            false
        }
        Err(_) => {
            defmt::warn!("mqtt: publish to '{}' timed out", topic);
            false
        }
    }
}

/// Format and publish `{"open":<bool>}` on `garagedoor/onchange`.
async fn publish_state<'buf, IO: minimq::Io>(
    conn: &mut Connection<'_, 'buf, IO>,
    state: DoorState,
) -> bool {
    let mut buf = [0u8; PAYLOAD_MAX];
    let Ok(payload) = mqtt::format_onchange(&mut buf, is_open(state)) else {
        defmt::warn!("mqtt: onchange payload truncated");
        return false;
    };
    let ok = publish_payload(conn, mqtt::TOPIC_ONCHANGE, payload).await;
    if ok {
        defmt::info!("mqtt: onchange published, open={}", is_open(state));
    }
    ok
}

/// Format and publish a correlated status response on `garagedoor/status/response`.
///
/// Returns `false` only when the underlying MQTT transport publish fails or
/// times out (indicating the session should be torn down); malformed UUIDs or
/// oversized payloads are logged and discarded without dropping the session.
async fn publish_status_response<'buf, IO: minimq::Io>(
    conn: &mut Connection<'_, 'buf, IO>,
    current: Option<DoorState>,
    uuid_bytes: &[u8],
) -> bool {
    let open = match current {
        Some(state) => is_open(state),
        None => {
            defmt::warn!("mqtt: status before state known; replying closed");
            false
        }
    };
    let Ok(uuid) = core::str::from_utf8(uuid_bytes) else {
        defmt::warn!("mqtt: status uuid not valid utf8; discarding");
        return true;
    };
    let mut buf = [0u8; PAYLOAD_MAX];
    let Ok(payload) = mqtt::format_status_response(&mut buf, uuid, open, crate::VERSION) else {
        defmt::warn!("mqtt: status response payload truncated");
        return true;
    };
    publish_payload(conn, mqtt::TOPIC_STATUS_RESPONSE, payload).await
}

/// Wait out the current backoff and double it, capped at [`MAX_BACKOFF`].
async fn backoff_wait(backoff: &mut Duration) {
    Timer::after(*backoff).await;
    *backoff = core::cmp::min(*backoff * 2, MAX_BACKOFF);
}

/// Reset the MCU if no healthy MQTT session has been established within
/// [`NO_IP_REBOOT_MS`] and no OTA flash write is in progress.
fn maybe_reboot(watchdog: &NoIpWatchdog) {
    if watchdog.reboot_due(now_ms()) && !crate::watchdog::is_ota_flashing() {
        defmt::error!(
            "mqtt: no healthy session for {}s; resetting MCU",
            NO_IP_REBOOT_MS / 1000
        );
        crate::watchdog::trigger_reset();
    }
}

/// Record one failed connect attempt and wait out the backoff. After repeated
/// failures the link is assumed associated-but-dead and a Wi-Fi rejoin is
/// requested, since the link state alone never reports that condition. If no
/// healthy MQTT session has been established within [`NO_IP_REBOOT_MS`], reset
/// the MCU to recover the radio/network stack.
async fn fail_backoff(backoff: &mut Duration, rejoin: &mut RejoinCounter, watchdog: &NoIpWatchdog) {
    maybe_reboot(watchdog);
    if rejoin.failure() {
        defmt::warn!("mqtt: repeated connect failures; requesting Wi-Fi rejoin");
        WIFI_REJOIN_SIGNAL.signal(());
    }
    backoff_wait(backoff).await;
}

/// Resolve `host` via DNS with a bounded timeout.
async fn resolve_broker(stack: NetStack, host: &str) -> Option<embassy_net::IpAddress> {
    match with_timeout(
        Duration::from_millis(MQTT_IO_TIMEOUT_MS),
        stack.dns_query(host, DnsQueryType::A),
    )
    .await
    {
        Ok(Ok(addrs)) => match addrs.first() {
            Some(addr) => Some(*addr),
            None => {
                defmt::warn!("mqtt: broker '{}' resolved to no addresses", host);
                None
            }
        },
        Ok(Err(err)) => {
            defmt::warn!(
                "mqtt: DNS lookup for '{}' failed: {:?}",
                host,
                defmt::Debug2Format(&err)
            );
            None
        }
        Err(_) => {
            defmt::warn!("mqtt: DNS lookup for '{}' timed out", host);
            None
        }
    }
}

/// Drive an established MQTT session until a poll or publish fails.
async fn run_session<'buf, IO: minimq::Io>(
    conn: &mut Connection<'_, 'buf, IO>,
    current: &mut Option<DoorState>,
    backoff: &mut Duration,
    rejoin: &mut RejoinCounter,
    watchdog: &mut NoIpWatchdog,
) {
    let connected_at_ms = now_ms();
    let mut idle_polls: u32 = 0;

    loop {
        crate::watchdog::note_telemetry_alive();
        // The inbound publish borrows `conn`; scope it so the borrow ends
        // before any `conn.publish(...)` below.
        let action = {
            let next = select(
                with_timeout(Duration::from_millis(MQTT_POLL_TIMEOUT_MS), conn.poll()),
                DOOR_STATE_SIGNAL.wait(),
            )
            .await;
            match next {
                Either::First(Ok(Ok(Some(message)))) => {
                    *backoff = INITIAL_BACKOFF;
                    rejoin.success();
                    watchdog.note_last_good(now_ms());
                    if message.retained() {
                        defmt::warn!("mqtt: ignoring retained message on '{}'", message.topic());
                        Action::Unknown
                    } else {
                        classify(message.topic(), message.payload())
                    }
                }
                Either::First(Ok(Ok(None))) => {
                    idle_polls = idle_polls.saturating_add(1);
                    let now = now_ms();
                    if mqtt_session_healthy(connected_at_ms, now, idle_polls, false) {
                        *backoff = INITIAL_BACKOFF;
                        rejoin.success();
                        watchdog.note_last_good(now);
                    }
                    Action::Unknown
                }
                Either::First(Ok(Err(err))) => {
                    defmt::warn!("mqtt: poll failed: {:?}", defmt::Debug2Format(&err));
                    Action::Disconnected
                }
                Either::First(Err(_)) => {
                    defmt::warn!("mqtt: poll timed out; disconnecting");
                    Action::Disconnected
                }
                Either::Second(state) => Action::StateChange(state),
            }
        };

        match action {
            Action::Disconnected => break,
            Action::Trigger => {
                DOOR_CMD_CHANNEL.send(DoorCommand::Trigger).await;
                defmt::info!("mqtt: door trigger received");
            }
            Action::Reset => {
                RESET_REQUEST_SIGNAL.signal(());
                defmt::info!("mqtt: OTA reset requested");
            }
            Action::StateChange(state) => {
                *current = Some(state);
                if !publish_state(conn, state).await {
                    break;
                }
            }
            Action::Status { uuid, uuid_len } => {
                if !publish_status_response(conn, *current, &uuid[..uuid_len]).await {
                    break;
                }
            }
            Action::Unknown => {}
        }
    }
}

/// MQTT telemetry and control task.
#[embassy_executor::task]
pub async fn telemetry_task(stack: NetStack) -> ! {
    // Declared once so the session's buffer borrow stays stable across reconnects.
    let mut mqtt_rx = [0u8; MQTT_RX_LEN];
    let mut mqtt_tx = [0u8; MQTT_TX_LEN];
    let mut tcp_rx = [0u8; TCP_RX_LEN];
    let mut tcp_tx = [0u8; TCP_TX_LEN];

    let config = match ConfigBuilder::new(Buffers::new(&mut mqtt_rx, &mut mqtt_tx))
        .keepalive_interval(KEEPALIVE_SECS)
        .client_id(CLIENT_ID)
    {
        Ok(config) => config,
        Err(_) => {
            defmt::error!("mqtt: static client id rejected; telemetry disabled");
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    let mut session = Session::new(config);

    let mut backoff = INITIAL_BACKOFF;
    let mut rejoin = RejoinCounter::new();
    let mut watchdog = NoIpWatchdog::new();
    let mut current: Option<DoorState> = None;

    loop {
        crate::watchdog::note_telemetry_alive();
        if with_timeout(
            Duration::from_millis(DHCP_TIMEOUT_MS),
            stack.wait_config_up(),
        )
        .await
        .is_err()
        {
            maybe_reboot(&watchdog);
            continue;
        }

        let (host, port) = parse_broker(secrets::MQTT_BROKER);
        let Some(addr) = resolve_broker(stack, host).await else {
            fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
            continue;
        };

        let mut socket = TcpSocket::new(stack, &mut tcp_rx, &mut tcp_tx);
        socket.set_timeout(Some(CONNECT_TIMEOUT));
        match with_timeout(
            Duration::from_millis(MQTT_IO_TIMEOUT_MS),
            socket.connect((addr, port)),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                defmt::warn!(
                    "mqtt: TCP connect to {}:{} failed: {:?}",
                    host,
                    port,
                    defmt::Debug2Format(&err)
                );
                fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
                continue;
            }
            Err(_) => {
                defmt::warn!("mqtt: TCP connect to {}:{} timed out", host, port);
                fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
                continue;
            }
        }
        socket.set_keep_alive(Some(Duration::from_millis(MQTT_TCP_KEEPALIVE_MS)));
        socket.set_timeout(Some(Duration::from_millis(MQTT_TCP_TIMEOUT_MS)));

        let mut conn = match with_timeout(
            Duration::from_millis(MQTT_IO_TIMEOUT_MS),
            session.connect(socket),
        )
        .await
        {
            Ok(Ok(conn)) => conn,
            Ok(Err(err)) => {
                defmt::warn!(
                    "mqtt: session connect failed: {:?}",
                    defmt::Debug2Format(&err)
                );
                fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
                continue;
            }
            Err(_) => {
                defmt::warn!("mqtt: session connect timed out");
                fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
                continue;
            }
        };
        defmt::info!("mqtt: connected to {}:{}", host, port);

        let topics = [
            TopicFilter::new(mqtt::TOPIC_TRIGGER),
            TopicFilter::new(mqtt::TOPIC_STATUS),
            TopicFilter::new(mqtt::TOPIC_RESET),
        ];
        match with_timeout(
            Duration::from_millis(MQTT_IO_TIMEOUT_MS),
            conn.subscribe(&topics, &[]),
        )
        .await
        {
            Ok(Ok(_)) => defmt::info!("mqtt: subscribed to command topics"),
            Ok(Err(err)) => {
                defmt::warn!("mqtt: subscribe failed: {:?}", defmt::Debug2Format(&err));
                fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
                continue;
            }
            Err(_) => {
                defmt::warn!("mqtt: subscribe timed out");
                fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
                continue;
            }
        }

        // Refresh the cached state and publish the legacy initial snapshot.
        if let Some(state) = DOOR_STATE_SIGNAL.try_take() {
            current = Some(state);
        }
        if let Some(state) = current {
            if !publish_state(&mut conn, state).await {
                fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
                continue;
            }
        }

        run_session(
            &mut conn,
            &mut current,
            &mut backoff,
            &mut rejoin,
            &mut watchdog,
        )
        .await;

        defmt::info!(
            "mqtt: connection lost; reconnecting in {}s",
            backoff.as_secs()
        );
        fail_backoff(&mut backoff, &mut rejoin, &watchdog).await;
    }
}
