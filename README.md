# garagedoorcontroller — Pico W firmware (native Rust / embassy)

## Overview

The garage door controller firmware: a reed switch reports the door position, a
relay drives the opener, telemetry and control run over MQTT, and the device
updates itself over the air. This repository is the **Pico W firmware**, written
in native Rust on `embassy-rp` for the RP2040 (`thumbv6m-none-eabi`). It is a
Cargo workspace of three crates: `core` (host-testable policy), `app` (the
firmware image), and `bootloader` (the `embassy-boot-rp` A/B bootloader).

## Prerequisites

- The pinned Rust toolchain from `rust-toolchain.toml` (channel **1.98.1**);
  `rustup show` installs it on first use. It includes `rustfmt`, `clippy` and
  `llvm-tools-preview`, and pins the `thumbv6m-none-eabi` target. If the target
  is missing, add it explicitly: `rustup target add thumbv6m-none-eabi`.
- `cargo-binutils` for `rust-objcopy` and `cargo size` (used by `deploy.sh` and
  the CI size gate): `cargo install cargo-binutils --locked`.
- For release publishing: `ssh`, `scp`, `mosquitto_pub`, and `sha256sum`
  (coreutils).

A clean checkout must materialise the (gitignored) secrets file before it
builds — CI does the same:

```sh
cp app/secrets.example.rs app/src/secrets.rs
```

## Build / test / lint

These mirror `.github/workflows/ci.yml`. The host target must be explicit
because `.cargo/config.toml` sets a global `[build] target = thumbv6m-none-eabi`.

```sh
cargo fmt --all --check
cargo clippy -p garagedoor-core --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
cargo test -p garagedoor-core --target x86_64-unknown-linux-gnu
cargo build --release --target thumbv6m-none-eabi --workspace
```

Size budget (enforced by CI via `cargo size`, text + data):

- `garagedoor-app` ≤ **798,720 bytes** (780 KiB, the ACTIVE-slot flash limit).
- `garagedoor-bootloader` ≤ **24,320 bytes**.

## Release / OTA publishing

The published version is the single bare integer in the repo-root `VERSION`
file. Bumping it is an explicit repository commit; `deploy.sh` only reads
`VERSION` and never increments it.

Release procedure — edit `VERSION`, commit, then deploy:

```sh
# edit VERSION, e.g. 12 -> 13
git add VERSION && git commit -m "release: VERSION 13"
./deploy.sh
```

`deploy.sh` is fully environment-driven. These are required:

| Variable | Meaning |
|---|---|
| `DEPLOY_PUBLISH_HOST` | `ssh`/`scp` target, e.g. `user@host` |
| `DEPLOY_PUBLISH_PATH` | remote base directory (nginx firmware root), e.g. `/path/to/firmware` |
| `DEPLOY_PROJECT` | remote project sub-directory; must match the firmware's `OTA_PROJECT`, e.g. `garagedoor` |
| `DEPLOY_MQTT_BROKER` | MQTT broker host for the reset trigger, e.g. `broker` |

These are optional, with their defaults:

| Variable | Default |
|---|---|
| `DEPLOY_MQTT_PORT` | unset — `mosquitto_pub`'s own port |
| `DEPLOY_MQTT_USER` | unset (never echoed) |
| `DEPLOY_MQTT_PASSWORD` | unset (never echoed) |
| `DEPLOY_TRIGGER_TOPIC` | `garagedoor/reset` |
| `DEPLOY_TRIGGER_PAYLOAD` | `reset` |

`DEPLOY_PROJECT` must match the firmware's `OTA_PROJECT`. The remote layout
under `$DEPLOY_PUBLISH_PATH/$DEPLOY_PROJECT` is `version`,
`${version}.bin`, and `${version}.bin.sha256`.

Targets must never be committed — this is a public repository. Instead of
exporting the variables, `deploy.sh` also loads an env file:
`$SCRIPT_DIR/.env.deploy` if it exists, or the path given by
`DEPLOY_ENV_FILE`. Example `.env.deploy` (placeholders only):

```sh
DEPLOY_PUBLISH_HOST=user@host
DEPLOY_PUBLISH_PATH=/path/to/firmware
DEPLOY_PROJECT=garagedoor
DEPLOY_MQTT_BROKER=broker
# optional
DEPLOY_MQTT_PORT=1883
DEPLOY_MQTT_USER=user
DEPLOY_MQTT_PASSWORD=secret
DEPLOY_TRIGGER_TOPIC=garagedoor/reset
DEPLOY_TRIGGER_PAYLOAD=reset
```

`.env.deploy` is gitignored; restrict it to your user:

```sh
chmod 600 .env.deploy
```

Precedence: exported environment variables override values from the file, so
`DEPLOY_PROJECT=other ./deploy.sh` beats the file's `DEPLOY_PROJECT`.

Dry run — builds, extracts and hashes locally so the printed hash is real, but
copies and publishes nothing and does not trigger the device:

```sh
./deploy.sh --dry-run
```

## Flashing & recovery

The flash layout is a bootloader (`boot2` + bootloader at `0x10000000`) plus the
ACTIVE application at `0x10006000`. The bootloader must be built **alone** so that
workspace feature unification does not strip `.boot2`:

```sh
cargo build --release --target thumbv6m-none-eabi -p garagedoor-bootloader
cargo build --release --target thumbv6m-none-eabi -p garagedoor-app
```

### SWD (probe-rs)

```
probe-rs download --chip RP2040 --binary-format bin --base-address 0x10000000 bootloader.bin
probe-rs download --chip RP2040 --binary-format bin --base-address 0x10006000 app.bin
probe-rs reset   --chip RP2040
probe-rs attach  --chip RP2040 target/thumbv6m-none-eabi/release/garagedoor-app   # defmt RTT
```

On rigs where more than one debug port is visible on the SWD multi-drop,
`probe-rs` cannot auto-detect the chip; pass `--chip RP2040` explicitly.

### USB / UF2

The RP2040 bootrom requires a **contiguous** UF2 (a 256-byte payload on every
block, and block addresses contiguous from the load address). `scripts/make_uf2.py`
builds one from the bootloader + application bins:

```sh
python3 scripts/make_uf2.py gdc.uf2 0x10000000 0x0 bootloader.bin 0x6000 app.bin
# copy gdc.uf2 onto the RPI-RP2 mass-storage volume
```

To enter the ROM bootloader without the BOOTSEL button, the running bootloader
honours the `DfuDetach` state (`Updater::mark_dfu()`); on the next reset it calls
`reset_to_usb_boot`.
