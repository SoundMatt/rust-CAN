# rust-CAN

A Rust library for [CAN bus](https://en.wikipedia.org/wiki/CAN_bus) (Controller Area Network) communication.
Works in automotive, industrial, robotics, and heavy-vehicle domains.

The `Bus` trait is stable. Implementations are swappable without changing application code.

[![CI](https://github.com/SoundMatt/rust-CAN/actions/workflows/ci.yml/badge.svg)](https://github.com/SoundMatt/rust-CAN/actions/workflows/ci.yml)

**RELAY spec:** v1.1 · **Safety:** ASIL-B (ISO 26262) · **Language:** Rust 2021

---

## Crates

| Module | Description | Platform |
|---|---|---|
| `rust_can` | Core `Bus` trait, `Frame`, `Filter`, validation | All |
| `virtual_bus` | In-process broadcast bus — zero OS dependencies | All |
| `mock` | Mock bus for unit testing with frame injection | All |
| `socketcan` | Linux SocketCAN — hardware and virtual CAN interfaces | Linux |
| `dbc` | DBC file parser and signal decoder | All |
| `isotp` | ISO 15765-2 (ISO-TP) multi-frame transport | All |
| `j1939` | SAE J1939 — 29-bit extended ID, PGN addressing | All |
| `safety` | E2E protection — DataID, SourceID, SequenceCounter, CRC-16 | All |
| `adapt` | RELAY v1.1 adapter — `Adapt()`, `ToMessage()`, `FromMessage()` | All |

---

## Install

```toml
[dependencies]
rust-can = { git = "https://github.com/SoundMatt/rust-CAN" }
tokio = { version = "1", features = ["full"] }
```

---

## Quick start

```rust
use std::sync::Arc;
use rust_can::{virtual_bus::VirtualBus, Bus, Frame};
use rust_can::relay::{Context, SubscriberOptions};

#[tokio::main]
async fn main() {
    let bus = Arc::new(VirtualBus::new());

    let rx = bus.subscribe(vec![], SubscriberOptions::default()).await.unwrap();

    bus.send(Context::background(), Frame {
        id: 0x100,
        data: vec![0xDE, 0xAD, 0xBE, 0xEF],
        ..Default::default()
    }).await.unwrap();

    let frame = rx.recv().await.unwrap();
    println!("{:03X}#{}", frame.id, hex::encode(&frame.data)); // 100#deadbeef

    bus.close().await.unwrap();
}
```

---

## Switching transports

```rust
// Development / testing — zero dependencies:
use rust_can::virtual_bus::VirtualBus;
let bus = Arc::new(VirtualBus::new());

// Linux hardware or vcan:
#[cfg(target_os = "linux")]
use rust_can::socketcan::SocketCanBus;
#[cfg(target_os = "linux")]
let bus = Arc::new(SocketCanBus::new("vcan0").unwrap());

// Application code only references the Bus trait.
```

---

## DBC signal decoding

```rust
use rust_can::dbc;

let db = dbc::parse(r#"
BO_ 256 EngineStatus: 8 Vector__XXX
 SG_ EngineSpeed : 0|16@1+ (0.5,0) [0|8000] "rpm" Vector__XXX
"#).unwrap();

let values = db.decode(0x100, &[0x00, 0x10, 0, 0, 0, 0, 0, 0]);
println!("{} rpm", values["EngineSpeed"]);
```

---

## ISO-TP (multi-frame messages)

```rust
use rust_can::isotp::{IsoTpConn, Config};
use rust_can::relay::Context;
use std::time::Duration;

let cfg = Config {
    tx_id: 0x7E0,
    rx_id: 0x7E8,
    ..Default::default()
};

let conn = IsoTpConn::new(bus.clone(), cfg).unwrap();
conn.send(Context::background(), &payload).await.unwrap();
let data = conn.recv(Context::with_timeout(Duration::from_millis(500))).await.unwrap();
```

---

## J1939

```rust
use rust_can::j1939::{J1939Bus, Pgn};
use rust_can::relay::SubscriberOptions;

let j_bus = J1939Bus::new(bus.clone(), 0x00);  // source address 0x00
let pgn = Pgn(0x0FECA);                         // CCVS PGN
let rx = j_bus.subscribe(pgn, SubscriberOptions::default()).await.unwrap();
j_bus.send(Context::background(), rust_can::j1939::J1939Frame {
    priority: 6.into(),
    pgn,
    src: 0x00,
    dst: 0xFF,
    data: payload.to_vec(),
}).await.unwrap();
```

---

## Safety E2E protection

```rust
use rust_can::safety::{Config, Protector, Receiver};

let cfg = Config { data_id: 0x0001, source_id: 0x0010 };
let protector = Protector::new(cfg.clone());
let receiver  = Receiver::new(cfg);

// Wrap payload before sending (use with ISO-TP or CAN FD):
let protected = protector.protect(&[0x01, 0x02, 0x03]);

// On receive (after ISO-TP reassembly):
let original = receiver.unwrap(&protected).unwrap();
```

The 10-byte header (DataID, SourceID, SequenceCounter, CRC-16) does not fit in a standard 8-byte
CAN frame. Use with ISO-TP or CAN FD.

---

## RELAY adapter

```rust
use rust_can::adapt::adapt;
use rust_can::relay::{Node, Context, Message};
use std::sync::Arc;

let node = adapt(Arc::new(VirtualBus::new()));
node.send(Context::background(), Message {
    protocol: rust_can::relay::Protocol::CAN,
    id: "256".into(),
    payload: vec![0x01, 0x02],
    ..Default::default()
}).await.unwrap();
```

---

## Docker quickstart

```bash
docker compose -f docker/docker-compose.yml up --build
```

---

## CLI (rust-can)

```bash
rust-can version --format json
rust-can capabilities
rust-can status --format json
rust-can send --id 0x100 --data DEADBEEF
rust-can subscribe --count 10
```

---

## ASIL-B compliance

rust-CAN targets **ASIL-B** under ISO 26262 Part 6.

| Activity | Tool | Output |
|---|---|---|
| Coding standard lint | `rsfusa lint` | `lint-report.json` |
| Static analysis | `rsfusa analyze` | `check-report.json` |
| Requirement trace | `rsfusa trace` | `trace.json` |
| FMEA | `fmea.json` (pre-populated) | — |
| Threat analysis | `tara.json` (pre-populated) | — |
| Complexity V(G) | `rsfusa comp` | `comp-report.json` |
| Tool qualification | `rsfusa qualify` | `qualify-report.json` |
| SBOM | `rsfusa release` | `sbom.json` |

CI enforces `rsfusa check --strict` — any ERROR finding fails the build.

---

## Philosophy

- **Trait-first** — one stable `Bus` trait; transports are swappable.
- **Safety-oriented** — ASIL-B E2E protection built-in; rust-FuSa in CI.
- **Testable by default** — the virtual bus needs no OS support; tests run anywhere.
- **Async-native** — fully async via Tokio; no blocking in the hot path.
- **RELAY-conformant** — interoperable with go-CAN, cpp-CAN, and other x-Net libraries.

---

## License

Mozilla Public License v2.0. Copyright (c) 2026 Matt Jones.
