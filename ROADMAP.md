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
- [x] RELAY v2.0 adapter — Adapt(), ToMessage(), FromMessage()
- [x] CLI binary `rust-can` — version, capabilities, status, send, subscribe
- [x] ASIL-B safety evidence — FMEA, TARA, safety case, rsfusa CI
- [x] RELAY conformance — spec v2.0

## v0.2.0 — Robustness

- [x] OBD-II (ISO 15031) over ISO-TP
- [x] UDS (ISO 14229) over ISO-TP
- [ ] CAN XL frame support in SocketCAN
- [ ] J1939 multi-packet TP (BAM + CMDT)
- [ ] candump log record/replay in CLI
- [ ] ASIL-C evidence gap analysis
- [ ] `relay conform` CLI integration tests
- [x] CAN interop testing — live two-process self-interop + can-utils
      third-party validator, both over a real kernel `vcan0` interface
      (see "Interop testing" section below)

## v0.3.0 — Ecosystem

- [ ] `relay-rs` crate dependency (replace bundled relay module)
- [ ] Published to crates.io as `rust-can`
- [ ] SOME/IP bridge adapter
- [ ] DDS bridge adapter
- [ ] Docker image published to ghcr.io/soundmatt/rust-can

## Interop testing — a real gap beyond RELAY's `interop` command

RELAY's `relay interop` (spec §11.2.1) is **not** a wire-level test. What it
actually does: for each golden vector, it runs `<binary> convert --protocol
CAN --format json` and checks that the resulting `relay.Message` JSON is
byte-identical against the in-process reference. That's a semantic/JSON-level
equivalence check on the RELAY-adapter boundary (`adapt.rs`,
`to_message`/`from_message`) — it never puts a frame on a real CAN bus and
says nothing about whether `socketcan::SocketCanBus`'s real `AF_CAN` socket
encode/decode path actually interoperates with anything outside this crate's
own process.

Unlike rust-DDS (which needed a live two-process harness *and* a third,
independent RTPS stack — CycloneDDS — to avoid both sides sharing the same
misreading of the RTPS spec), CAN doesn't need a third-party network stack
the way DDS does: Linux's own kernel SocketCAN subsystem plus `can-utils`
(`candump`/`cangen`/`cansend`) already *is* the independent, real wire — the
kernel's `vcan0` net device broadcasts real CAN frames to every socket bound
to it, and `can-utils` is a separate, mature, widely-deployed C codebase that
never goes through any of this crate's own encode/decode logic.

**Done — both deliverables.** Landed in
[rust-CAN#28](https://github.com/SoundMatt/rust-CAN/pull/28) as a new
`can-interop` CI job (ubuntu-only, probe-then-skip-cleanly if `vcan`/
`can-utils` are ever unavailable on a runner — mirrors rust-DDS's own
`cyclone-interop` job posture), separate from and in addition to the
`conformance` job above:

- **Live two-process self-interop** — `can-interop-peer`
  (`src/bin/can_interop_peer.rs`, a `[[bin]]` target, not part of the public
  library API), a standalone sender/receiver process driven entirely by the
  real, production `rust_can::socketcan::SocketCanBus` (real `AF_CAN`
  socket, real `write(2)`/`read(2)` frame I/O). `tests/can_two_process_interop.rs`
  spawns two of these as separate OS processes bound to the same real
  `vcan0` interface and asserts every frame the sender transmitted matches,
  field for field (arbitration ID, DLC/data length, data content, and —
  for CAN FD — the `fd`/`brs`/`esi` flags), a frame the receiver decoded
  from the real kernel socket, across both a classic-CAN standard-ID run and
  an extended-ID CAN FD run that varies DLC and the BRS/ESI flags across the
  frames it sends. `#[ignore]`d in the default `cargo test` sweep (needs
  Linux + a pre-existing `vcan0`); runs in the `can-interop` CI job via
  `cargo test --release --test can_two_process_interop -- --ignored
  --test-threads=1`.
- **Third-party `can-utils` cross-validation** — `tests/can_thirdparty_interop.rs`
  (gated behind the `can-utils-interop` Cargo feature, mirroring rust-DDS's
  `cyclone-interop` feature gate) reuses `can-interop-peer` unmodified in
  both directions: `cangen` injects real frames onto `vcan0` for a
  `can-interop-peer --role receiver` process to decode, independently
  cross-checked against a concurrently-running `candump -L vcan0` capture
  (the ground truth of what `cangen` actually sent, since its output is
  random by default); and the reverse, a `can-interop-peer --role sender`
  process sends deterministic frames while `candump -L vcan0` independently
  captures the real wire bytes, asserted field-exact against the sender's
  own report. Runs in the `can-interop` CI job via `cargo test --release
  --features can-utils-interop --test can_thirdparty_interop -- --ignored
  --test-threads=1`, gated on both `vcan0` *and* `can-utils` being
  available on the runner.
