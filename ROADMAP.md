# rust-CAN Roadmap

## v0.1.0 — Foundation (current)

- [x] Core types: Frame, Filter, LoanedFrame, validate_frame
- [x] Bus trait (async, Send+Sync)
- [x] Optional traits: LoaningBus, HealthProvider, MetricsProvider, Drainer
- [x] VirtualBus — in-process, zero dependencies
- [x] MockBus — unit-testing with frame injection
- [x] SocketCAN — Linux kernel AF_CAN socket (Linux-only)
- [x] ISO-TP (ISO 15765-2) — multi-frame transport
- [x] J1939 — SAE J1939 PGN addressing over 29-bit extended IDs
- [x] DBC parser — signal decode
- [x] E2E safety — CRC-16/CCITT-FALSE, sequence counter
- [x] RELAY v1.1 adapter — Adapt(), ToMessage(), FromMessage()
- [x] CLI binary `rust-can` — version, capabilities, status, send, subscribe
- [x] ASIL-B safety evidence — FMEA, TARA, safety case, rsfusa CI
- [x] RELAY conformance — spec v1.1

## v0.2.0 — Robustness

- [ ] OBD-II (ISO 15031) over ISO-TP
- [ ] UDS (ISO 14229) over ISO-TP
- [ ] CAN XL frame support in SocketCAN
- [ ] J1939 multi-packet TP (BAM + CMDT)
- [ ] candump log record/replay in CLI
- [ ] ASIL-C evidence gap analysis
- [ ] `relay conform` CLI integration tests

## v0.3.0 — Ecosystem

- [ ] `relay-rs` crate dependency (replace bundled relay module)
- [ ] Published to crates.io as `rust-can`
- [ ] SOME/IP bridge adapter
- [ ] DDS bridge adapter
- [ ] Docker image published to ghcr.io/soundmatt/rust-can
