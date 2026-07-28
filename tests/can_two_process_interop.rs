// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! The live two-process CAN interop test harness — `ROADMAP.md`'s
//! "Interop testing" section, deliverable 1 of 2: two real rust-CAN
//! processes, bound to the same real kernel `vcan0` SocketCAN interface,
//! one sending real CAN frames via the RELAY-conformant `Bus` API, the
//! other receiving and verifying field-exact correctness (arbitration ID,
//! DLC/data length, data content, and — for CAN FD — the `fd`/`brs`/`esi`
//! flags).
//!
//! Each test here spawns two real, independent OS processes of the
//! `can-interop-peer` binary (`src/bin/can_interop_peer.rs` — real
//! `rust_can::socketcan::SocketCanBus`, no test-only shortcuts) on a real
//! `vcan0` interface and asserts, from each process's own JSON report on
//! stdout, that every frame the sender transmitted matches, field for
//! field, a frame the receiver decoded from the real kernel socket.
//!
//! This mirrors rust-DDS's `tests/rtps_two_process_interop.rs` as closely
//! as CAN's transport model allows. The key structural difference: RTPS
//! needs SPDP/SEDP discovery-completion checks before either side can be
//! trusted to have "found" the other; SocketCAN has no discovery protocol
//! at all — a raw `AF_CAN` socket simply receives whatever the kernel
//! broadcasts on the bound interface from the moment it is bound onward.
//! So instead of an in-band "discovered" signal, this harness relies on
//! process sequencing (start the receiver first, give it a head start to
//! open+bind its socket, then start the sender — see
//! `run_sender_and_receiver`) plus `can-interop-peer`'s own `--settle-ms`
//! delay before its first send, exactly the two-layer defence
//! `rtps_two_process_interop.rs` uses for its own (differently-caused)
//! discovery-vs-data race.
//!
//! `#[ignore]`d by default (spawning real OS processes that open real
//! `AF_CAN` kernel sockets requires Linux and a pre-existing `vcan0`
//! interface — unsuited to the default cross-platform `cargo test`
//! matrix). Run explicitly, after bringing up `vcan0`:
//!
//! ```text
//! sudo modprobe vcan
//! sudo ip link add dev vcan0 type vcan
//! sudo ip link set up vcan0
//! cargo test --release --test can_two_process_interop -- --ignored --test-threads=1
//! ```
//!
//! Also runs, gated the same way, in the `can-interop` CI job
//! (`.github/workflows/ci.yml`, ubuntu-only).

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Absolute path to the `can-interop-peer` binary Cargo built for this
/// test binary — see the `CARGO_BIN_EXE_<name>` docs
/// (https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates).
fn peer_bin() -> &'static str {
    env!("CARGO_BIN_EXE_can-interop-peer")
}

/// The real SocketCAN interface these tests run against. Overridable via
/// `CAN_INTEROP_IFACE` for local runs against a differently-named vcan
/// interface; defaults to `vcan0`, matching this repo's own
/// `README.md`/`ROADMAP.md` convention and the `can-interop` CI job.
fn iface() -> String {
    std::env::var("CAN_INTEROP_IFACE").unwrap_or_else(|_| "vcan0".to_string())
}

/// True when the real network interface named by `iface()` does not exist
/// on this host — the signal this file treats as "vcan0 was never brought
/// up" and skips cleanly on, rather than failing with a confusing socket
/// error deep inside a spawned peer process. Defense in depth on top of
/// the `can-interop` CI job's own probe-then-skip step: this lets anyone
/// who force-runs these `--ignored` tests without `vcan0` set up get a
/// clear skip notice instead of a panic.
fn vcan_iface_present() -> bool {
    std::path::Path::new(&format!("/sys/class/net/{}", iface())).exists()
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct FrameRecord {
    id: u32,
    ext: bool,
    #[allow(dead_code)]
    rtr: bool,
    fd: bool,
    brs: bool,
    esi: bool,
    dlc: usize,
    data_hex: String,
}

#[derive(Debug, serde::Deserialize)]
struct Report {
    #[allow(dead_code)]
    role: String,
    ok: bool,
    #[allow(dead_code)]
    iface: String,
    #[allow(dead_code)]
    count: usize,
    sent: Option<Vec<FrameRecord>>,
    received: Option<Vec<FrameRecord>>,
    error: Option<String>,
}

/// Spawns `peer_bin()` with `args`, waits up to `deadline` (polling
/// `try_wait` rather than blocking indefinitely, so a hung child cannot
/// hang this test), and parses the last stdout line as a [`Report`].
fn run_peer(args: &[&str], deadline: Duration) -> Report {
    let mut child = Command::new(peer_bin())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", peer_bin()));

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if start.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "peer process (args={args:?}) did not exit within {deadline:?} — killed. \
                 This should not happen: can-interop-peer bounds its own runtime via \
                 --recv-timeout-secs."
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");

    let last_line = stdout
        .lines()
        .last()
        .unwrap_or_else(|| panic!("peer (args={args:?}) printed no stdout.\nstderr:\n{stderr}"));
    serde_json::from_str(last_line).unwrap_or_else(|e| {
        panic!(
            "peer (args={args:?}) last stdout line was not valid JSON: {e}\nline: {last_line}\n\
             exit status: {status:?}\nstderr:\n{stderr}"
        )
    })
}

/// Spawns the receiver first (as a background `std::thread`, since
/// `run_peer` blocks until the child exits) so it has already opened and
/// bound its real `AF_CAN` socket before the sender's first write —
/// SocketCAN has no discovery protocol to wait out (see this file's
/// module doc comment), so process sequencing plus the sender's own
/// `--settle-ms` is the whole defence against this race, not a hand-shake
/// signal either side can observe.
fn run_sender_and_receiver(
    sender_args: Vec<String>,
    receiver_args: Vec<String>,
    deadline: Duration,
) -> (Report, Report) {
    let receiver_handle = std::thread::spawn(move || {
        let args: Vec<&str> = receiver_args.iter().map(String::as_str).collect();
        run_peer(&args, deadline)
    });
    // Give the receiver a small head start to open+bind its socket before
    // the sender's first real frame hits the wire.
    std::thread::sleep(Duration::from_millis(150));
    let args: Vec<&str> = sender_args.iter().map(String::as_str).collect();
    let sender_report = run_peer(&args, deadline);
    let receiver_report = receiver_handle.join().expect("receiver thread panicked");
    (sender_report, receiver_report)
}

//fusa:test REQ-CAN-001
//fusa:test REQ-CAN-009
#[test]
#[ignore = "spawns real OS processes that open real AF_CAN kernel sockets on vcan0; run via the can-interop CI job"]
fn classic_standard_id_frames_flow_end_to_end_field_exact() {
    if !vcan_iface_present() {
        eprintln!(
            "can_two_process_interop: {} not present — skipping (bring it up: sudo modprobe vcan && sudo ip link add dev {} type vcan && sudo ip link set up {})",
            iface(), iface(), iface()
        );
        return;
    }

    let deadline = Duration::from_secs(30);
    let iface = iface();
    let (sender, receiver) = run_sender_and_receiver(
        vec![
            "--role".into(),
            "sender".into(),
            "--iface".into(),
            iface.clone(),
            "--id".into(),
            "291".into(), // 0x123
            "--count".into(),
            "8".into(),
            "--seed".into(),
            "7".into(),
            "--interval-ms".into(),
            "10".into(),
        ],
        vec![
            "--role".into(),
            "receiver".into(),
            "--iface".into(),
            iface,
            "--count".into(),
            "8".into(),
            "--recv-timeout-secs".into(),
            "20".into(),
        ],
        deadline,
    );

    assert!(sender.ok, "sender did not report success: {sender:?}");
    assert!(receiver.ok, "receiver did not report success: {receiver:?}");

    let sent = sender.sent.expect("sender report missing `sent`");
    let received = receiver
        .received
        .expect("receiver report missing `received`");
    assert_eq!(sent.len(), 8);
    assert_eq!(received.len(), 8);
    assert_eq!(
        sent, received,
        "field-exact mismatch between what the sender transmitted and what the receiver \
         decoded from the real vcan0 kernel interface"
    );
    // Sanity: every frame really did use the standard-ID, classic-CAN
    // shape this test asked for, and DLC genuinely varied across the run
    // (proving this isn't a fixed-length coincidence).
    for f in &sent {
        assert_eq!(f.id, 291);
        assert!(!f.ext && !f.fd && !f.brs && !f.esi);
    }
    let dlcs: Vec<usize> = sent.iter().map(|f| f.dlc).collect();
    assert_eq!(dlcs, vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

//fusa:test REQ-CAN-010
//fusa:test REQ-CAN-013
#[test]
#[ignore = "spawns real OS processes that open real AF_CAN kernel sockets on vcan0; run via the can-interop CI job"]
fn extended_id_can_fd_frames_flow_end_to_end_fd_flags_exact() {
    if !vcan_iface_present() {
        eprintln!(
            "can_two_process_interop: {} not present — skipping (bring it up: sudo modprobe vcan && sudo ip link add dev {} type vcan && sudo ip link set up {})",
            iface(), iface(), iface()
        );
        return;
    }

    let deadline = Duration::from_secs(30);
    let iface = iface();
    let (sender, receiver) = run_sender_and_receiver(
        vec![
            "--role".into(),
            "sender".into(),
            "--iface".into(),
            iface.clone(),
            "--id".into(),
            "1750890".into(), // arbitrary value > 0x7FF, needs --ext
            "--ext".into(),
            "--fd".into(),
            "--count".into(),
            "12".into(),
            "--seed".into(),
            "42".into(),
            "--interval-ms".into(),
            "10".into(),
        ],
        vec![
            "--role".into(),
            "receiver".into(),
            "--iface".into(),
            iface,
            "--count".into(),
            "12".into(),
            "--recv-timeout-secs".into(),
            "20".into(),
        ],
        deadline,
    );

    assert!(sender.ok, "sender did not report success: {sender:?}");
    assert!(receiver.ok, "receiver did not report success: {receiver:?}");

    let sent = sender.sent.expect("sender report missing `sent`");
    let received = receiver
        .received
        .expect("receiver report missing `received`");
    assert_eq!(sent.len(), 12);
    assert_eq!(received.len(), 12);
    assert_eq!(
        sent, received,
        "field-exact mismatch between what the sender transmitted and what the receiver \
         decoded from the real vcan0 kernel interface (extended ID + CAN FD run)"
    );

    for f in &sent {
        assert_eq!(f.id, 1750890);
        assert!(f.ext && f.fd);
    }
    // brs/esi genuinely varied across the run (see can_interop_peer's
    // build_frame: brs on even indices, esi on indices divisible by 3),
    // so this run actually exercises both flags in both states, not just
    // one fixed combination.
    assert!(sent.iter().any(|f| f.brs));
    assert!(sent.iter().any(|f| !f.brs));
    assert!(sent.iter().any(|f| f.esi));
    assert!(sent.iter().any(|f| !f.esi));
    let dlcs: Vec<usize> = sent.iter().map(|f| f.dlc).collect();
    assert_eq!(dlcs, (1..=12).collect::<Vec<usize>>());
}

#[test]
#[ignore = "spawns real OS processes that open real AF_CAN kernel sockets on vcan0; run via the can-interop CI job"]
fn peer_binary_reports_failure_when_no_sender_ever_appears() {
    if !vcan_iface_present() {
        eprintln!(
            "can_two_process_interop: {} not present — skipping (bring it up: sudo modprobe vcan && sudo ip link add dev {} type vcan && sudo ip link set up {})",
            iface(), iface(), iface()
        );
        return;
    }

    // Sanity check on the harness itself: a lone receiver with a short
    // timeout and nobody sending must report ok=false with an empty
    // `received`, not hang or false-positive.
    let report = run_peer(
        &[
            "--role",
            "receiver",
            "--iface",
            &iface(),
            "--count",
            "1",
            "--recv-timeout-secs",
            "2",
        ],
        Duration::from_secs(10),
    );
    assert!(!report.ok);
    assert!(report
        .received
        .expect("receiver report missing `received`")
        .is_empty());
    assert!(report.error.is_some());
}
