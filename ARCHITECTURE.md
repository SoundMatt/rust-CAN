# Software Architecture Description

**Document:** rust-CAN Software Architecture  
**Version:** 0.1.0  
**Standard:** ISO 26262:2018 Part 6 §6.6 (Software Architectural Design)  
**ASIL:** ASIL-B (safety partition) / QM (protocol layers)

---

## 1. Purpose

This document satisfies ISO 26262-6 §6.6.1 (software architectural design) and provides the
module decomposition, interface specifications, data flows, and ASIL allocation required for
an ASIL-B safety case.

---

## 2. Module Decomposition

```
rust-CAN crate
├── frame          — Frame, Filter types; validate_frame()          [ASIL-B]
├── error          — Error enum (Closed, InvalidFrame, Timeout …)   [ASIL-B]
├── bus            — Bus trait; SubInner; FrameReceiver              [ASIL-B]
├── safety         — Protector, Receiver (E2E CRC-16 + seq counter) [ASIL-B]
├── virtual_bus    — In-process broadcast bus for testing/simulation [QM]
├── mock           — Programmable mock bus for unit tests            [QM]
├── socketcan      — Linux AF_CAN SocketCAN transport (Linux only)   [QM]
├── relay          — RELAY v1.1 types (Context, Message, Protocol)   [QM]
├── adapt          — RELAY adapter (adapt, to_message, from_message) [QM]
├── isotp          — ISO 15765-2 multi-frame transport               [QM→ASIL-A*]
├── j1939          — SAE J1939 protocol layer                        [QM]
├── dbc            — DBC file parser and signal decoder              [QM]
├── crc            — CRC-16/CCITT-FALSE primitive                    [ASIL-B]
└── bin/main       — CLI entry point (rust-can)                      [QM]
```

\* `isotp` timeout behaviour is classified ASIL-A (see SG-CAN-03 in `.fusa-hara.json`).

---

## 3. ASIL Allocation and Partitioning

| Module | ASIL | Rationale |
|--------|------|-----------|
| `frame` | ASIL-B | validate_frame() is the sole enforcement point for SG-CAN-01 |
| `error` | ASIL-B | Error discriminants must be distinct; misclassification could suppress safety actions |
| `bus` | ASIL-B | Bus trait Send+Sync bounds guarantee concurrent safety (REQ-CAN-006) |
| `safety` | ASIL-B | CRC + sequence counter are the E2E safety mechanism for SG-CAN-02 |
| `crc` | ASIL-B | CRC primitive is shared by the safety module |
| `isotp` | ASIL-A | Timeout boundary (SG-CAN-03); functional correctness is QM |
| All others | QM | Protocol and test utilities; no safety goal dependency |

**Freedom from interference:** ASIL-B and QM partitions share the `Bus` trait object boundary.
The `Bus` trait requires `Send + Sync`; Rust's ownership rules statically prevent data races
across the boundary at compile time. No additional runtime monitoring is required.

---

## 4. Module Interface Specifications

### 4.1 `frame` module

| Export | Signature | Pre-condition | Post-condition |
|--------|-----------|---------------|----------------|
| `Frame` | struct | — | All fields public; default() produces a valid standard frame with id=0 |
| `Filter` | struct | — | `id` and `mask` are u32 |
| `validate_frame` | `(&Frame) → Result<(), Error>` | Frame is any value | Returns Ok iff frame satisfies all RELAY §15.1 constraints; returns `Error::InvalidFrame` otherwise |

### 4.2 `bus` module

| Export | Signature | Pre-condition | Post-condition |
|--------|-----------|---------------|----------------|
| `Bus::send` | `(Context, Frame) → Result<(), Error>` | Bus not closed; frame pre-validated | Frame transmitted; returns `Error::Closed` if bus closed |
| `Bus::subscribe` | `(Vec<Filter>, SubscriberOptions) → Result<FrameReceiver, Error>` | Bus not closed | Returns a `FrameReceiver` that receives matching frames; returns `Error::Closed` if bus closed |
| `Bus::close` | `() → Result<(), Error>` | — | Bus is closed; all subscribers receive channel close; idempotent |

### 4.3 `safety` module

| Export | Signature | Pre-condition | Post-condition |
|--------|-----------|---------------|----------------|
| `Protector::protect` | `(&[u8]) → Vec<u8>` | Any payload | Returns payload prefixed by 10-byte E2E header; sequence counter advanced by 1 |
| `Receiver::unwrap` | `(&[u8]) → Result<Vec<u8>, E2EError>` | Any byte slice | Returns payload slice (bytes 10+) on success; returns `E2EError` with kind CrcMismatch, SequenceGap, or HeaderTooShort on failure |

### 4.4 `crc` module

| Export | Signature | Pre-condition | Post-condition |
|--------|-----------|---------------|----------------|
| `crc16_ccitt_false` | `(&[u8]) → u16` | Any byte slice | Returns CRC-16/CCITT-FALSE (poly=0x1021, init=0xFFFF, no reflection, no XOR-out) |

---

## 5. Data Flow Diagram

```
[Application]
     │ payload
     ▼
[safety::Protector]──build_header──▶[crc::crc16_ccitt_false]
     │ protected = [header ++ payload]
     ▼
[Bus::send]──validate_frame──▶[frame::validate_frame]
     │ Frame{data: protected}
     ▼
[Transport] (VirtualBus | SocketCAN | MockBus)
     │ Frame delivered to subscribers
     ▼
[Bus subscriber channel]
     │ Frame
     ▼
[safety::Receiver]──verify CRC + seq──▶ Ok(payload) | Err(E2EError)
     │
     ▼
[Application]
```

---

## 6. Shared Data and Concurrency

| Shared resource | Protection mechanism | ASIL |
|---|---|---|
| `VirtualBus::inner` (subscriber list) | `tokio::sync::Mutex` | QM |
| `SocketCanBus::subscribers` | `tokio::sync::Mutex` | QM |
| `SocketCanBus::closed` | `AtomicBool` (SeqCst) | QM |
| `Protector::seq` | `AtomicU32` (SeqCst) | ASIL-B |
| `Receiver::state` | `std::sync::Mutex` | ASIL-B |

All Bus trait implementations are `Send + Sync` (enforced by the trait bound). The Rust
compiler statically guarantees the absence of data races across these boundaries.

---

## 7. External Interfaces

| Interface | Protocol | Direction | Notes |
|---|---|---|---|
| SocketCAN kernel socket | AF_CAN / SOCK_RAW | Bidirectional | Linux only; `cfg(target_os = "linux")` |
| RELAY message bus | RELAY v1.1 | Bidirectional | Via `adapt()` wrapper |
| DBC file | Text (line-oriented) | Input | Parsed in-memory; no file I/O |

---

## 8. Security Architecture

### 8.1 Threat boundary

rust-CAN is a library, not a standalone system. The threat boundary is the CAN bus interface — all frames arriving from the bus are treated as potentially attacker-controlled until validated by `validate_frame()` and (optionally) the E2E layer.

### 8.2 Security control mapping

| Threat (tara.json) | Module | Control |
|---|---|---|
| T-CAN-01: frame injection | `frame` | `validate_frame()` gates every send path |
| T-CAN-02: payload tampering | `safety` | CRC-16 (safety); `MessageAuthenticator` trait (security) |
| T-CAN-03: replay | `safety` | 32-bit monotonic sequence counter in E2E header |
| T-CAN-04: bus-off DoS | `error`, `socketcan` | `Error::BusOff` exposed; recovery is application responsibility |
| T-CAN-05: node impersonation | `safety` | `MessageAuthenticator` trait (caller provides keyed MAC) |
| T-CAN-06: eavesdropping | — | **Risk accepted** — CAN is broadcast; no encryption in scope |
| T-CAN-07: frame flooding | `bus` | `SubscriberOptions::rate_limit_per_sec` enforced in `SubInner::push()` |
| T-CAN-08: ISO-TP exhaustion | `isotp` | `tokio::time::timeout` on all waits |
| T-CAN-09: DBC crash | `dbc` | All parse paths return `Err`; no `unwrap()` on external input |

### 8.3 CRC-16 scope boundary

The CRC-16/CCITT-FALSE in the `safety` module is an **ISO 26262 safety** control, not an **ISO/SAE 21434 security** control. The distinction:

- **Safety** (random faults): CRC-16 has Hamming distance ≥ 4 for messages ≤ 32767 bits. Satisfies ASIL-B requirements for E2E protection.
- **Security** (adversarial forgery): CRC-16 has no keying material. An observer can compute a valid CRC for any forged payload. Does NOT satisfy CAL-3.

For security-level integrity, callers must implement `safety::MessageAuthenticator` using HMAC-SHA256 (≥ 256-bit key) or AES-128-CMAC with an HSM-managed key.

## 9. Known Architectural Constraints

1. **SocketCAN is Linux-only.** macOS and Windows builds exclude `socketcan` and `libc`
   dependencies via `cfg(target_os = "linux")`.
2. **CAN XL hardware.** CAN XL frame support in SocketCAN requires Linux ≥ 6.0 with
   XL-capable drivers; rust-CAN defines the frame type and validates constraints but cannot
   guarantee hardware delivery.
3. **No persistent state between process restarts.** The E2E sequence counter starts at 0
   on each Protector construction. Applications requiring counter continuity across restarts
   must persist and restore the counter externally.
