# garagedoorcontroller — Pico W Firmware (Native Rust / Embassy)

[![CI](https://github.com/retsimx/garagedoorcontroller/actions/workflows/ci.yml/badge.svg)](https://github.com/retsimx/garagedoorcontroller/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Production-grade embedded firmware for the Raspberry Pi Pico W garage door controller. Built with native asynchronous Rust on [`embassy-rp`](https://github.com/embassy-rs/embassy) for the RP2040 microcontroller (`thumbv6m-none-eabi`).

The firmware monitors door position using a magnetic reed switch, drives the garage opener relay through hardware-interlocked pulse logic, streams telemetry and status over MQTT, and supports self-testing, fail-safe Over-The-Air (OTA) updates using a dual-bank bootloader.

---

## Table of Contents

- [1. Architecture Overview](#1-architecture-overview)
  - [Flash Memory Layout](#flash-memory-layout)
  - [Embassy Async Tasks](#embassy-async-tasks)
- [2. Hardware Pinout & Wiring](#2-hardware-pinout--wiring)
  - [Pico W Pinout Diagram](#pico-w-pinout-diagram)
  - [Wiring & Interface Table](#wiring--interface-table)
  - [Hardware Considerations](#hardware-considerations)
- [3. Prerequisites & Environment Setup](#3-prerequisites--environment-setup)
  - [Rust Toolchain](#rust-toolchain)
  - [Host Dependencies](#host-dependencies)
  - [Secrets Configuration](#secrets-configuration)
- [4. Building & Testing](#4-building--testing)
  - [Workspace Structure](#workspace-structure)
  - [Commands & Quality Checks](#commands--quality-checks)
  - [Firmware Size Budgets](#firmware-size-budgets)
- [5. Initial Provisioning (First Flash)](#5-initial-provisioning-first-flash)
  - [Building Standalone Binaries](#building-standalone-binaries)
  - [Method A: SWD Programming via probe-rs](#method-a-swd-programming-via-probe-rs)
  - [Method B: USB Mass Storage via Contiguous UF2](#method-b-usb-mass-storage-via-contiguous-uf2)
- [6. Over-The-Air (OTA) Deployment](#6-over-the-air-ota-deployment)
  - [Version Management](#version-management)
  - [Deployment Workflow](#deployment-workflow)
  - [Configuration (.env.deploy)](#configuration-envdeploy)
  - [Dry-Run Verification](#dry-run-verification)
  - [Self-Test & Automated Rollback](#self-test--automated-rollback)
- [7. MQTT Reference & Wire Contracts](#7-mqtt-reference--wire-contracts)
  - [Topic Summary](#topic-summary)
  - [Message Schemas & Specifications](#message-schemas--specifications)
  - [Correlation UUID Rules](#correlation-uuid-rules)
- [8. License](#8-license)

---

## 1. Architecture Overview

The system runs entirely `no_std` bare-metal Rust on dual ARM Cortex-M0+ cores. It uses `embassy-executor` for cooperative async multitasking, `cyw43` for on-board Wi-Fi and Bluetooth communication, and `embassy-boot-rp` for robust A/B firmware swapping.

```
┌────────────────────────────────────────────────────────────────────────┐
│                        RP2040 Microcontroller                          │
│                                                                        │
│   ┌────────────────┐      ┌─────────────────┐      ┌───────────────┐   │
│   │ Actuator Task  │      │   Sensor Task   │      │ Watchdog Task │   │
│   │ (Relay pulse / │      │  (Reed debounce │      │ (8s hardware  │   │
│   │   interlock)   │      │   door state)   │      │  supervision) │   │
│   └───────▲────────┘      └────────┬────────┘      └───────────────┘   │
│           │ DoorCommand            │ DoorState                         │
│           │ Channel                │ Signal                            │
│   ┌───────┴────────────────────────▼────────┐      ┌───────────────┐   │
│   │             Telemetry Task              │      │   OTA Task    │   │
│   │   (minimq MQTT client / state pub /     │◄────►│ (HTTPS stream │   │
│   │            query response)              │Reset │  DFU client)  │   │
│   └───────────────────────▲─────────────────┘Signal└───────┬───────┘   │
│                           │                                │           │
│   ┌───────────────────────▼────────────────────────────────▼───────┐   │
│   │                  embassy-net Network Stack                     │   │
│   │          CYW43 PIO Wi-Fi Driver & Station Supervisor           │   │
│   └────────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────────┘
```

### Flash Memory Layout

The Raspberry Pi Pico W contains 2 MiB (2,048 KiB) of external QSPI NOR flash (`W25Q16JV`). The memory map is partitioned into dedicated bootloader, active application, DFU staging, and persistent state sectors:

| Flash Address Range | Offset | Size | Partition | Description |
|---|---|---|---|---|
| `0x10000000` – `0x10005FFF` | `+0x000000` | 24 KiB | `garagedoor-bootloader` | Second-stage bootrom header (`.boot2`) and `embassy-boot-rp` engine |
| `0x10006000` – `0x100C8FFF` | `+0x006000` | 780 KiB | `garagedoor-app` (Slot 0 / ACTIVE) | Currently running application image (linked at `0x10006000`) |
| `0x100C9000` – `0x1018CFFF` | `+0x0C9000` | 784 KiB (780 KiB payload + 4 KiB trailer) | Slot 1 (DFU Staging) | Staging slot where new OTA firmware updates are streamed and verified |
| `0x1018D000` – `0x1018DFFF` | `+0x18D000` | 4 KiB | State & Swap Progress | Non-volatile swap state, transaction markers, and page progress table |
| `0x1018E000` – `0x101FFFFF` | `+0x18E000` | 456 KiB | Reserved Flash | Unallocated flash reserve |

*Note: The ACTIVE partition provides 780 KiB of application payload capacity. The DFU partition is 784 KiB (780 KiB payload + one 4 KiB sector for embassy-boot trailer bookkeeping).*

### Embassy Async Tasks

The application runtime (`garagedoor-app`) coordinates independent tasks via `embassy-sync` channels and signals:

1. **Actuator Task (`app/src/actuator.rs`)**:
   - Manages the relay control output on GPIO 18.
   - Enforces an active HIGH pulse of 500 ms followed by a mandatory 500 ms interlock cooldown period.
   - Rejects extraneous triggers during an active pulse or cooldown to prevent rapid cycling or mechanical damage to the opener.
2. **Sensor Task (`app/src/sensor.rs`)**:
   - Continuously monitors the magnetic reed switch input on GPIO 21.
   - Implements a software debounce filter (20 ms window) to reject mechanical bounce and line noise.
   - Signals committed state changes to the telemetry system only when a stable level transition is verified.
3. **Telemetry Task (`app/src/telemetry.rs`)**:
   - Maintains an asynchronous MQTT 3.1.1 session using `minimq` over the `embassy-net` TCP stack.
   - Subscribes to inbound command topics (`garagedoor/trigger`, `garagedoor/status`, `garagedoor/reset`).
   - Dispatches state changes to `garagedoor/onchange` and answers correlated status queries on `garagedoor/status/response`.
   - Handles network drops with exponential backoff (1 s to 30 s) without blocking local door actuation.
4. **OTA Task (`app/src/ota/`)**:
   - Listens for update requests triggered via MQTT `garagedoor/reset` or bootloader evaluation.
   - Connects to the remote firmware server via HTTPS using `embedded-tls`, streaming chunked binaries directly into the DFU flash partition.
   - Verifies the cryptographic SHA-256 digest against the published `.bin.sha256` checksum before marking the partition for swap.
5. **Wi-Fi / Radio Supervisor (`app/src/wifi.rs`, `app/src/radio.rs`)**:
   - Drives the Infineon CYW43439 Wi-Fi chip over PIO SPI.
   - Monitors station connection state, re-associates upon link failure, and supervises DHCP lease renewals.
6. **Watchdog Task (`app/src/watchdog.rs`)**:
   - Arms the RP2040 hardware watchdog for an 8-second interval before peripheral initialization.
   - Runs a periodic feeder loop. If an unrecoverable hang or failed OTA self-test occurs, the watchdog triggers a hard reset, allowing the bootloader to roll back to the previously verified active image.

---

## 2. Hardware Pinout & Wiring

### Pico W Pinout Diagram

```
                             Raspberry Pi Pico W
                                ┌─────────────┐
        Optional UART0 TX (GP0) │ 1   [USB] 40│ VBUS (5V DC Input)
        Optional UART0 RX (GP1) │ 2         39│ VSYS
                         Ground │ 3         38│ Ground
                            GP2 │ 4         37│ 3V3_EN
                            GP3 │ 5         36│ 3V3(OUT)
                            GP4 │ 6         35│ ADC_VREF
                            GP5 │ 7         34│ GP28
                         Ground │ 8         33│ Ground
                            GP6 │ 9         32│ GP27
                            GP7 │ 10        31│ GP26
                            GP8 │ 11        30│ RUN (Reset)
                            GP9 │ 12        29│ GP22
                         Ground │ 13        28│ Ground (Reed Return)
                           GP10 │ 14        27│ GP21 (Reed Switch Input)
                           GP11 │ 15        26│ GP20
                           GP12 │ 16        25│ GP19
                           GP13 │ 17        24│ GP18 (Relay Control Output)
                         Ground │ 18        23│ Ground
                           GP14 │ 19        22│ GP17
                           GP15 │ 20        21│ GP16
                                └──────┬──────┘
                                  [DEBUG / SWD]
                                  │   │   │
                              SWCLK  GND  SWDIO
```

### Wiring & Interface Table

| Pin Number | Signal / Pin | Direction | Connected Device | Function & Behavior |
|---|---|---|---|---|
| **Pin 24** | **GPIO 18** | Output | Relay Driver Module `IN` | **Door Actuator**: Active HIGH 500 ms pulse triggers opener dry contacts. Safe Low on power-up. |
| **Pin 27** | **GPIO 21** | Input | Magnetic Reed Switch | **Door Sensor**: Bare input (no internal pull). External pull network determines level.<br>• **HIGH**: Door Closed<br>• **LOW**: Door Open |
| **Pin 28** | **GND** | Power | Magnetic Reed Switch Return | Common digital ground reference for sensor loop. |
| **Pin 1** | **GP0** (UART0 TX)| Output | Serial Console / Logic Analyzer| Optional UART debug output (115200 baud, 8N1). |
| **Pin 2** | **GP1** (UART0 RX)| Input | Serial Console | Optional UART RX (reserved). |
| **Pin 39** | **VSYS** | Power | 5V DC Supply | Main system power input (1.8V to 5.5V). |
| **SWD** | **SWCLK / SWDIO / GND** | Bidirectional | CMSIS-DAP Debug Probe | Hardware flashing via `probe-rs`, debugging, and high-speed `defmt RTT` event logging. |

### Hardware Considerations

- **Relay Driving**: GPIO 18 must drive a transistor or optocoupled relay module. Do not connect a raw relay coil directly to the RP2040 GPIO pin.
- **Reed Switch Circuit**: The firmware configures GPIO 21 as `Pull::None` (matching legacy MicroPython behavior). Ensure an external pull-up or pull-down resistor circuit is present so the line does not float when the reed switch opens.
- **Relay Pulse Interlock**: The software enforces a strict 500 ms cooldown after every pulse. Even if repeated MQTT trigger commands are received, the relay cannot be energized continuously or pulsed rapidly.

---

## 3. Prerequisites & Environment Setup

### Rust Toolchain

The project requires the Rust toolchain version pinned in `rust-toolchain.toml` (**1.98.1**).

Ensure the bare-metal Cortex-M0+ cross-compilation target is installed:

```sh
rustup target add thumbv6m-none-eabi
rustup component add rustfmt clippy llvm-tools-preview
```

### Host Dependencies

Install `cargo-binutils` for binary extraction and flash size inspection:

```sh
cargo install cargo-binutils --locked
```

For deployment and initial hardware provisioning:
- **`probe-rs`**: For flashing and RTT logging over SWD (`cargo install probe-rs-tools --locked`).
- **`python3`**: For building contiguous UF2 files using `scripts/make_uf2.py`.
- **`mosquitto_pub`**: For issuing MQTT deployment triggers.
- **Standard coreutils**: `ssh`, `scp`, `sha256sum`, `chmod`.

### Secrets Configuration

A fresh checkout must initialize the firmware secrets file before building. Copy the template:

```sh
cp app/secrets.example.rs app/src/secrets.rs
```

Edit `app/src/secrets.rs` with your network credentials and endpoint parameters:
- `WIFI_SSID` & `WIFI_PASSWORD`
- `MQTT_BROKER` (e.g. `mqtt://192.168.1.50:1883`)
- `OTA_URL` (e.g. `https://firmware.local/firmware`)
- `OTA_PROJECT` (default: `garagedoor`)
- `OTA_USER` & `OTA_PASSWORD` (HTTP Basic Auth for OTA server)

*Note: `app/src/secrets.rs` is explicitly gitignored and must never be committed.*

---

## 4. Building & Testing

### Workspace Structure

The project is organized as a Cargo workspace with three member crates:
- `core` (`garagedoor-core`): Pure, `no_std`, zero-hardware business logic (state debouncing, relay interlock, MQTT JSON codecs, flash arithmetic). Tested natively on host x86_64.
- `app` (`garagedoor-app`): The main embedded application binary for the RP2040.
- `bootloader` (`garagedoor-bootloader`): The A/B bootloader image containing `.boot2` and `embassy-boot-rp`.

### Commands & Quality Checks

Run the standard CI validation suite locally:

```sh
# 1. Format check
cargo fmt --all --check

# 2. Host linting & unit tests (garagedoor-core on host target)
cargo clippy -p garagedoor-core --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
cargo test -p garagedoor-core --target x86_64-unknown-linux-gnu

# 3. Embedded linting (workspace cross-check)
cargo clippy --workspace --target thumbv6m-none-eabi -- -D warnings

# 4. Release compilation
cargo build --release --target thumbv6m-none-eabi --workspace
```

### Firmware Size Budgets

Flash slot boundaries are enforced by strict size limits. You can verify binary footprints using `cargo size`:

```sh
cargo size --release --target thumbv6m-none-eabi -p garagedoor-app -- -A
cargo size --release --target thumbv6m-none-eabi -p garagedoor-bootloader -- -A
```

- **`garagedoor-app`**: Max **798,720 bytes** (780 KiB usable ACTIVE partition).
- **`garagedoor-bootloader`**: Max **24,320 bytes** (23.75 KiB bootloader partition).

---

## 5. Initial Provisioning (First Flash)

On a new or wiped Raspberry Pi Pico W, both the bootloader and the initial application image must be programmed into flash.

### Building Standalone Binaries

> **Important**: Build the bootloader and application individually to prevent Cargo workspace feature unification from stripping `.boot2`:

```sh
cargo build --release --target thumbv6m-none-eabi -p garagedoor-bootloader
cargo build --release --target thumbv6m-none-eabi -p garagedoor-app
```

Extract raw binary files using `rust-objcopy`:

```sh
rust-objcopy -O binary target/thumbv6m-none-eabi/release/garagedoor-bootloader bootloader.bin
rust-objcopy -O binary target/thumbv6m-none-eabi/release/garagedoor-app app.bin
```

---

### Method A: SWD Programming via probe-rs

Connect a CMSIS-DAP debugger (e.g. Raspberry Pi Debug Probe) to the Pico W SWD header (`SWCLK`, `GND`, `SWDIO`).

Program the bootloader at `0x10000000` and the application at `0x10006000`:

```sh
# Program bootloader partition
probe-rs download --chip RP2040 --binary-format bin --base-address 0x10000000 bootloader.bin

# Program ACTIVE application partition
probe-rs download --chip RP2040 --binary-format bin --base-address 0x10006000 app.bin

# Reset the microcontroller
probe-rs reset --chip RP2040

# Attach real-time defmt RTT log viewer
probe-rs attach --chip RP2040 target/thumbv6m-none-eabi/release/garagedoor-app
```

---

### Method B: USB Mass Storage via Contiguous UF2

If an SWD probe is unavailable, flash using the RP2040 USB bootrom:

1. Hold the **BOOTSEL** button on the Pico W while plugging it into your host computer via USB. The board mounts as a mass-storage drive named `RPI-RP2`.
2. The RP2040 bootrom requires a **contiguous** UF2 image (every block must carry a 256-byte payload contiguous from the load address to ensure proper sector erasure). Generate the unified image using `scripts/make_uf2.py`:

```sh
python3 scripts/make_uf2.py gdc.uf2 0x10000000 0x0 bootloader.bin 0x6000 app.bin
```

3. Drag and drop or copy `gdc.uf2` onto the `RPI-RP2` drive:

```sh
cp gdc.uf2 /media/$USER/RPI-RP2/
```

The board will automatically unmount, write the flash sectors, and boot.

---

## 6. Over-The-Air (OTA) Deployment

Once the device is provisioned with `garagedoor-bootloader` and an initial `garagedoor-app`, all subsequent updates are distributed over the air.

### Version Management

The firmware version is defined as a single integer in the repository-root `VERSION` file (e.g. `12`). 
To initiate a release, bump the version and commit the change:

```sh
# Example: bump version from 12 to 13
echo "13" > VERSION
git add VERSION
git commit -m "release: VERSION 13"
```

### Deployment Workflow

Execute `./deploy.sh` to compile the release binary, generate the SHA-256 checksum, publish the artifacts to the firmware web server, and send an MQTT reset notification to the device:

```sh
./deploy.sh
```

### Configuration (.env.deploy)

Deployment parameters can be passed as environment variables or placed in an untracked `.env.deploy` file in the repository root:

```sh
# Copy template or create .env.deploy
cat <<'EOF' > .env.deploy
DEPLOY_PUBLISH_HOST=user@firmware-server.local
DEPLOY_PUBLISH_PATH=/var/www/firmware
DEPLOY_PROJECT=garagedoor
DEPLOY_MQTT_BROKER=192.168.1.50

# Optional settings
DEPLOY_MQTT_PORT=1883
DEPLOY_MQTT_USER=mqtt_user
DEPLOY_MQTT_PASSWORD=mqtt_secret
DEPLOY_TRIGGER_TOPIC=garagedoor/reset
DEPLOY_TRIGGER_PAYLOAD=reset
EOF

chmod 600 .env.deploy
```

| Variable | Requirement | Description |
|---|---|---|
| `DEPLOY_PUBLISH_HOST` | **Required** | SSH/SCP target host (`user@host`). |
| `DEPLOY_PUBLISH_PATH` | **Required** | Remote directory on web server serving firmware binaries. |
| `DEPLOY_PROJECT` | **Required** | Project sub-directory matching `OTA_PROJECT` (default: `garagedoor`). |
| `DEPLOY_MQTT_BROKER` | **Required** | MQTT broker host address for the reset notification. |
| `DEPLOY_MQTT_PORT` | Optional | MQTT broker port (defaults to standard `1883`). |
| `DEPLOY_MQTT_USER` | Optional | Username for MQTT authentication. |
| `DEPLOY_MQTT_PASSWORD` | Optional | Password for MQTT authentication. |
| `DEPLOY_TRIGGER_TOPIC` | Optional | MQTT topic to trigger OTA check (default: `garagedoor/reset`). |
| `DEPLOY_TRIGGER_PAYLOAD`| Optional | MQTT trigger payload (default: `reset`). |

The remote web server hosts the following files under `${DEPLOY_PUBLISH_PATH}/${DEPLOY_PROJECT}/`:
- `version`: Plaintext file containing the current integer version (e.g. `13`).
- `13.bin`: Raw firmware binary for version 13.
- `13.bin.sha256`: Hex-encoded SHA-256 checksum of `13.bin`.

### Dry-Run Verification

Test the build and packaging pipeline without uploading files or publishing MQTT messages:

```sh
./deploy.sh --dry-run
```

This compiles the release image, extracts the binary, calculates the cryptographic hash, and prints the exact remote operations that would be executed.

### Self-Test & Automated Rollback

The firmware employs a fail-safe update cycle:

```
                  ┌───────────────────────────────┐
                  │ 1. Download & Flash DFU Slot  │
                  └───────────────┬───────────────┘
                                  ▼
                  ┌───────────────────────────────┐
                  │   2. Bootloader Swap to DFU   │
                  └───────────────┬───────────────┘
                                  ▼
                  ┌───────────────────────────────┐
                  │    3. Boot & Run Self-Test    │
                  │  (Relay Low? Reed Readable?   │
                  │      Wi-Fi Connected?)        │
                  └───────┬───────────────┬───────┘
                     Pass │               │ Fail / Watchdog Panic
                          ▼               ▼
          ┌────────────────────────┐    ┌────────────────────────┐
          │  Mark Image Confirmed  │    │  Roll Back to Previous │
          │     (Boot State)       │    │     Working Version    │
          └────────────────────────┘    └────────────────────────┘
```

1. Upon receiving `garagedoor/reset`, the OTA task streams the new image into Slot 1 (DFU) and verifies its SHA-256 digest.
2. The partition swap flag is set, and the device restarts into `garagedoor-bootloader`.
3. The bootloader swaps the ACTIVE and DFU partitions and boots into the new firmware.
4. **Self-Test Verification**: On initial boot in swap mode, the application verifies:
   - Actuator relay is safely deasserted (Low).
   - Sensor reed switch is functional and readable.
   - Wi-Fi network link is successfully established within the self-test timeout.
5. If self-test passes, the firmware marks itself as permanently confirmed (`State::Boot`).
6. If self-test fails or an unhandled panic triggers the 8-second hardware watchdog, the MCU reboots. The bootloader detects unconfirmed state and rolls back to the previous known-good firmware slot.

---

## 7. MQTT Reference & Wire Contracts

### Topic Summary

| Topic | Direction | QoS | Purpose | Expected Payload |
|---|---|---|---|---|
| `garagedoor/trigger` | Inbound (Broker → Device) | 0 or 1 | Trigger door pulse | Any payload (triggers 500 ms relay pulse) |
| `garagedoor/status` | Inbound (Broker → Device) | 0 or 1 | Request current state | `{"uuid":"<correlation-id>"}` |
| `garagedoor/status/response` | Outbound (Device → Broker) | 0 | Correlated status response | `{"uuid":"<correlation-id>","result":{"open":<bool>,"version":<u32>}}` |
| `garagedoor/onchange` | Outbound (Device → Broker) | 0 | Real-time state event | `{"open":<bool>}` |
| `garagedoor/reset` | Inbound (Broker → Device) | 0 or 1 | Trigger OTA update check | Any payload (signals OTA worker) |

---

### Message Schemas & Specifications

#### 1. Door Trigger Command
- **Topic**: `garagedoor/trigger`
- **Direction**: Inbound
- **Behavior**: If the relay interlock is idle (no active pulse or cooldown), GPIO 18 is asserted HIGH for 500 ms, then returned to LOW. Triggers received during an active pulse or within 500 ms after a pulse are safely ignored.
- **Example Payload**:
  ```json
  {"action":"trigger"}
  ```
  *(Note: Any payload or empty message is accepted).*

#### 2. Status Query Request
- **Topic**: `garagedoor/status`
- **Direction**: Inbound
- **Behavior**: Prompts the device to return its current door position and running firmware version.
- **Schema**:
  ```json
  {
    "uuid": "<string: 1 to 64 ASCII characters>"
  }
  ```
- **Example Payload**:
  ```json
  {"uuid":"req-8f42b3c1-01"}
  ```

#### 3. Status Query Response
- **Topic**: `garagedoor/status/response`
- **Direction**: Outbound
- **Behavior**: Published in direct response to a valid `garagedoor/status` query. Echoes the request `uuid` verbatim.
- **Schema**:
  ```json
  {
    "uuid": "<string>",
    "result": {
      "open": <boolean>,
      "version": <integer>
    }
  }
  ```
- **Example Payload**:
  ```json
  {"uuid":"req-8f42b3c1-01","result":{"open":false,"version":13}}
  ```

#### 4. State Change Event Notification
- **Topic**: `garagedoor/onchange`
- **Direction**: Outbound
- **Behavior**: Automatically published whenever the reed switch transitions between Open and Closed states (after 20 ms debouncing).
- **Schema**:
  ```json
  {
    "open": <boolean>
  }
  ```
- **Example Payloads**:
  ```json
  {"open":true}
  ```
  ```json
  {"open":false}
  ```

#### 5. OTA Reset Trigger
- **Topic**: `garagedoor/reset`
- **Direction**: Inbound
- **Behavior**: Wakes the background OTA task to check the remote web server for newer firmware versions.
- **Example Payload**:
  ```
  reset
  ```

---

### Correlation UUID Rules

Status queries require a JSON payload containing a top-level `"uuid"` property:
- **Maximum Length**: 64 bytes.
- **Validation**: Payloads lacking a top-level `"uuid"` key or supplying an empty string are rejected without generating a response.
- **Zero Allocation**: The parser (`garagedoor-core`) parses the JSON token directly from the inbound buffer without heap allocation.
- **Verbatim Echo**: The UUID string is returned exactly as received in the outbound response.

---

## 8. License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for full details.
