//! MQTT telemetry and control task.
//!
//! Owns one long-lived `minimq` session over the `embassy-net` stack: it
//! reconnects with exponential backoff across network loss, subscribes to the
//! command topics (`trigger`/`status`/`reset`), publishes state changes on
//! `onchange`, and answers correlated status requests. Network, parse, and
//! publish failures are logged and recovered from — this task never resets or
//! panics the MCU, so offline door control stays fully functional.

use embassy_futures::select::{select, Either};
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use garagedoor_core::mqtt;
use garagedoor_core::wifi::RejoinCounter;
use garagedoor_core::{DoorCommand, DoorState};
use minimq::{
    Buffers, ConfigBuilder, ConnectEvent, Connection, Publication, QoS, Session, TopicFilter,
};

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

/// Bounds the TCP handshake only; cleared once connected so the idle socket is
/// not torn down between MQTT keepalive pings (minimq services keepalive itself).
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

/// Format and publish `{"open":<bool>}` on `garagedoor/onchange`.
async fn publish_state<'buf, IO: minimq::Io>(
    conn: &mut Connection<'_, 'buf, IO>,
    state: DoorState,
) {
    let mut buf = [0u8; PAYLOAD_MAX];
    match mqtt::format_onchange(&mut buf, is_open(state)) {
        Ok(payload) => {
            let publication =
                Publication::bytes(mqtt::TOPIC_ONCHANGE, payload.as_bytes()).qos(QoS::AtMostOnce);
            if let Err(err) = conn.publish(publication).await {
                defmt::warn!(
                    "mqtt: onchange publish failed: {:?}",
                    defmt::Debug2Format(&err)
                );
            } else {
                defmt::info!("mqtt: onchange published, open={}", is_open(state));
            }
        }
        Err(_) => defmt::warn!("mqtt: onchange payload truncated"),
    }
}

/// Wait out the current backoff and double it, capped at [`MAX_BACKOFF`].
async fn backoff_wait(backoff: &mut Duration) {
    Timer::after(*backoff).await;
    *backoff = core::cmp::min(*backoff * 2, MAX_BACKOFF);
}

/// Record one failed connect attempt and wait out the backoff. After repeated
/// failures the link is assumed associated-but-dead and a Wi-Fi rejoin is
/// requested, since the link state alone never reports that condition.
async fn fail_backoff(backoff: &mut Duration, rejoin: &mut RejoinCounter) {
    if rejoin.failure() {
        defmt::warn!("mqtt: repeated connect failures; requesting Wi-Fi rejoin");
        WIFI_REJOIN_SIGNAL.signal(());
    }
    backoff_wait(backoff).await;
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
    let mut current: Option<DoorState> = None;

    loop {
        stack.wait_config_up().await;

        let (host, port) = parse_broker(secrets::MQTT_BROKER);
        let addr = match stack.dns_query(host, DnsQueryType::A).await {
            Ok(addrs) => match addrs.first() {
                Some(addr) => *addr,
                None => {
                    defmt::warn!("mqtt: broker '{}' resolved to no addresses", host);
                    fail_backoff(&mut backoff, &mut rejoin).await;
                    continue;
                }
            },
            Err(err) => {
                defmt::warn!(
                    "mqtt: DNS lookup for '{}' failed: {:?}",
                    host,
                    defmt::Debug2Format(&err)
                );
                fail_backoff(&mut backoff, &mut rejoin).await;
                continue;
            }
        };

        let mut socket = TcpSocket::new(stack, &mut tcp_rx, &mut tcp_tx);
        socket.set_timeout(Some(CONNECT_TIMEOUT));
        if let Err(err) = socket.connect((addr, port)).await {
            defmt::warn!(
                "mqtt: TCP connect to {}:{} failed: {:?}",
                host,
                port,
                defmt::Debug2Format(&err)
            );
            fail_backoff(&mut backoff, &mut rejoin).await;
            continue;
        }
        socket.set_timeout(None);

        let mut conn = match session.connect(socket).await {
            Ok(conn) => conn,
            Err(err) => {
                defmt::warn!(
                    "mqtt: session connect failed: {:?}",
                    defmt::Debug2Format(&err)
                );
                fail_backoff(&mut backoff, &mut rejoin).await;
                continue;
            }
        };
        backoff = INITIAL_BACKOFF;
        rejoin.success();
        defmt::info!("mqtt: connected to {}:{}", host, port);

        if conn.connect_event() == ConnectEvent::Connected {
            let topics = [
                TopicFilter::new(mqtt::TOPIC_TRIGGER),
                TopicFilter::new(mqtt::TOPIC_STATUS),
                TopicFilter::new(mqtt::TOPIC_RESET),
            ];
            if let Err(err) = conn.subscribe(&topics, &[]).await {
                defmt::warn!("mqtt: subscribe failed: {:?}", defmt::Debug2Format(&err));
                fail_backoff(&mut backoff, &mut rejoin).await;
                continue;
            }
            defmt::info!("mqtt: subscribed to command topics");
        }

        // Refresh the cached state and publish the legacy initial snapshot.
        if let Some(state) = DOOR_STATE_SIGNAL.try_take() {
            current = Some(state);
        }
        if let Some(state) = current {
            publish_state(&mut conn, state).await;
        }

        loop {
            // The inbound publish borrows `conn`; scope it so the borrow ends
            // before any `conn.publish(...)` below.
            let action = {
                let next = select(conn.recv(), DOOR_STATE_SIGNAL.wait()).await;
                match next {
                    Either::First(Ok(message)) => {
                        if message.retained() {
                            defmt::warn!(
                                "mqtt: ignoring retained message on '{}'",
                                message.topic()
                            );
                            Action::Unknown
                        } else {
                            classify(message.topic(), message.payload())
                        }
                    }
                    Either::First(Err(err)) => {
                        defmt::warn!("mqtt: receive failed: {:?}", defmt::Debug2Format(&err));
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
                    current = Some(state);
                    publish_state(&mut conn, state).await;
                }
                Action::Status { uuid, uuid_len } => {
                    let open = match current {
                        Some(state) => is_open(state),
                        None => {
                            defmt::warn!("mqtt: status before state known; replying closed");
                            false
                        }
                    };
                    let uuid = match core::str::from_utf8(&uuid[..uuid_len]) {
                        Ok(uuid) => uuid,
                        Err(_) => {
                            defmt::warn!("mqtt: status uuid not valid utf8; discarding");
                            continue;
                        }
                    };

                    let mut buf = [0u8; PAYLOAD_MAX];
                    match mqtt::format_status_response(&mut buf, uuid, open, crate::VERSION) {
                        Ok(payload) => {
                            let publication =
                                Publication::bytes(mqtt::TOPIC_STATUS_RESPONSE, payload.as_bytes())
                                    .qos(QoS::AtMostOnce);
                            if let Err(err) = conn.publish(publication).await {
                                defmt::warn!(
                                    "mqtt: status response publish failed: {:?}",
                                    defmt::Debug2Format(&err)
                                );
                                break;
                            }
                        }
                        Err(_) => defmt::warn!("mqtt: status response payload truncated"),
                    }
                }
                Action::Unknown => {}
            }
        }

        defmt::info!(
            "mqtt: connection lost; reconnecting in {}s",
            backoff.as_secs()
        );
        fail_backoff(&mut backoff, &mut rejoin).await;
    }
}
