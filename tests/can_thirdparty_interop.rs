// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! CAN wire-compatibility tests against live, independent `can-utils`
//! processes — `ROADMAP.md`'s "Interop testing" section, deliverable 2 of
//! 2: a third-party oracle beyond rust-CAN self-interop.
//!
//! Unlike rust-DDS (which needed a second, independent *DDS stack*
//! (CycloneDDS) to avoid both sides sharing the same misreading of the
//! RTPS spec), CAN doesn't need a third-party network stack the way DDS
//! does: Linux's own kernel SocketCAN subsystem plus `can-utils`
//! (`candump`/`cangen`) already *is* the independent validator — the
//! kernel's `vcan0` net device is the real wire, and `can-utils` is a
//! separate, mature, widely-deployed C codebase that never goes through
//! any of this crate's own encode/decode logic. So this file, rather than
//! standing up a second DDS-like peer process, drives `can-utils`
//! directly:
//!
//! 1. **`cangen` → rust-CAN receiver**: `cangen` injects real CAN frames
//!    onto `vcan0`; a `can-interop-peer --role receiver` process (the
//!    same production `rust_can::socketcan::SocketCanBus` code path
//!    already proven in `tests/can_two_process_interop.rs` — reused
//!    unmodified, exactly the precedent rust-DDS's own
//!    `cyclone_interop.rs` set by reusing `rtps-interop-peer`) decodes
//!    them. `candump -L vcan0`, run concurrently as an independent
//!    ground-truth recorder, captures what `cangen` actually put on the
//!    wire (not what we assume `cangen`'s random-by-default output would
//!    be), and the test asserts the receiver's decoded frames match
//!    `candump`'s capture field-for-field.
//! 2. **rust-CAN sender → `candump`**: the reverse — a
//!    `can-interop-peer --role sender` process sends deterministic frames
//!    via this crate's own encoder while `candump -L vcan0` independently
//!    captures the real bytes on the wire, and the test asserts the
//!    captured frames match the sender's own report of what it sent.
//!
//! Gated behind the `can-utils-interop` Cargo feature (this file does not
//! compile without it, so it is absent from the normal `cargo test` sweep
//! and default CI, mirroring rust-DDS's `cyclone-interop` feature gate on
//! `tests/cyclone_interop.rs`) *and* `#[ignore]`d on every test function
//! (belt and suspenders, matching this crate's own
//! `tests/can_two_process_interop.rs` convention). Also skips cleanly
//! (prints a note to stderr, returns without failing) rather than
//! panicking when `vcan0` or the `cangen`/`candump` binaries are not
//! actually present — the same probe-then-skip posture the `can-interop`
//! CI job (`.github/workflows/ci.yml`) applies at the job level.
//!
//! # Quick start
//!
//! ```text
//! sudo modprobe vcan
//! sudo ip link add dev vcan0 type vcan
//! sudo ip link set up vcan0
//! sudo apt-get install -y can-utils
//! cargo test --release --features can-utils-interop --test can_thirdparty_interop -- --ignored --test-threads=1
//! ```

#![cfg(feature = "can-utils-interop")]

use std::io::{BufRead, BufReader, Read as _};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Absolute path to the `can-interop-peer` binary Cargo built for this
/// test binary. Identical helper to `tests/can_two_process_interop.rs`'s
/// — kept as a separate copy rather than shared via `tests/common/`
/// because Cargo integration-test binaries do not share compiled state,
/// same rationale rust-DDS's `cyclone_interop.rs` documents for its own
/// ~30-LOC duplication of `rtps_two_process_interop.rs`'s `run_peer`.
fn peer_bin() -> &'static str {
    env!("CARGO_BIN_EXE_can-interop-peer")
}

fn iface() -> String {
    std::env::var("CAN_INTEROP_IFACE").unwrap_or_else(|_| "vcan0".to_string())
}

fn vcan_iface_present() -> bool {
    std::path::Path::new(&format!("/sys/class/net/{}", iface())).exists()
}

/// True when `name` cannot be found on `PATH` — the signal this file
/// treats as "can-utils is not installed" and skips cleanly on, same
/// posture as `looks_like_no_live_peer` in rust-DDS's `cyclone_interop.rs`.
fn tool_missing(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
struct FrameRecord {
    id: u32,
    ext: bool,
    #[allow(dead_code)]
    rtr: bool,
    #[allow(dead_code)]
    fd: bool,
    #[allow(dead_code)]
    brs: bool,
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    error: Option<String>,
}

/// Spawns `peer_bin()` with `args`, waits up to `deadline`, and parses the
/// last stdout line as a [`Report`]. Identical in shape to
/// `tests/can_two_process_interop.rs`'s helper of the same name.
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
            panic!("peer process (args={args:?}) did not exit within {deadline:?} — killed.");
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

// ---------------------------------------------------------------------------
// candump -L capture + parsing
// ---------------------------------------------------------------------------

/// A running `candump -L <iface>` capture. `candump -L` prints one line
/// per real frame observed on the interface, e.g.
/// `(1700000000.123456) vcan0 123#DEADBEEF` for a 3-hex-digit (standard)
/// ID, or an 8-hex-digit ID for extended frames. Lines are collected on a
/// background thread into `lines` as they arrive.
struct CandumpCapture {
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
}

impl CandumpCapture {
    fn start(iface: &str) -> Self {
        let mut child = Command::new("candump")
            .args(["-L", iface])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn candump: {e}"));
        let stdout = child.stdout.take().expect("piped candump stdout");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines_clone = Arc::clone(&lines);
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                lines_clone.lock().expect("lines mutex").push(line);
            }
        });
        // Give candump a moment to actually open its own socket before the
        // caller starts generating traffic.
        std::thread::sleep(Duration::from_millis(300));
        Self { child, lines }
    }

    /// Stops the capture and returns every frame line parsed so far.
    fn stop_and_parse(mut self) -> Vec<FrameRecord> {
        // Give the last few frames time to be flushed through the pipe.
        std::thread::sleep(Duration::from_millis(300));
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The background reader thread exits once the pipe closes (kill
        // above); a short join-by-sleep is simpler than plumbing a
        // JoinHandle through for a test-only capture helper.
        std::thread::sleep(Duration::from_millis(100));
        self.lines
            .lock()
            .expect("lines mutex")
            .iter()
            .filter_map(|l| parse_candump_line(l))
            .collect()
    }
}

/// Parses one `candump -L` output line into a [`FrameRecord`]. Returns
/// `None` for lines this test suite doesn't need to handle (RTR frames —
/// neither `can-interop-peer`'s deterministic sender nor `cangen`'s
/// classic-frame defaults emit them — and anything that doesn't match the
/// expected `(ts) iface id#data` shape).
fn parse_candump_line(line: &str) -> Option<FrameRecord> {
    let mut fields = line.split_whitespace();
    let _timestamp = fields.next()?;
    let _iface = fields.next()?;
    let frame_field = fields.next()?;
    let (id_hex, data_hex) = frame_field.split_once('#')?;
    if data_hex.starts_with('R') {
        return None; // RTR frame — not produced by this suite's generators.
    }
    let id = u32::from_str_radix(id_hex, 16).ok()?;
    // candump zero-pads standard (11-bit) IDs to 3 hex digits and
    // extended (29-bit) IDs to 8 — the same width convention this test
    // suite's frames are built with (see can_interop_peer.rs / cangen's
    // own default ID width without -e).
    let ext = id_hex.len() > 3;
    let data = hex::decode(data_hex).ok()?;
    Some(FrameRecord {
        id,
        ext,
        rtr: false,
        fd: false,
        brs: false,
        esi: false,
        dlc: data.len(),
        data_hex: hex::encode(&data),
    })
}

fn skip_if_environment_unavailable() -> bool {
    if !vcan_iface_present() {
        eprintln!(
            "can_thirdparty_interop: {} not present — skipping (bring it up: sudo modprobe vcan && sudo ip link add dev {} type vcan && sudo ip link set up {})",
            iface(), iface(), iface()
        );
        return true;
    }
    if tool_missing("cangen") || tool_missing("candump") {
        eprintln!(
            "can_thirdparty_interop: cangen/candump not found on PATH — skipping (is can-utils installed? `sudo apt-get install -y can-utils`)"
        );
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Deliverable 2/2, test 1: cangen -> rust-CAN receiver
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-001
#[test]
#[ignore = "requires a real vcan0 interface and can-utils installed; run via the can-interop CI job"]
fn cangen_frames_are_decoded_field_exact_by_socketcan_receiver() {
    if skip_if_environment_unavailable() {
        return;
    }
    let iface_name = iface();
    const N: usize = 20;

    let capture = CandumpCapture::start(&iface_name);

    // Start the receiver in the background so it's already listening
    // before cangen's first frame; cangen itself is fast (N * 5ms gap),
    // so give the receiver generous headroom beyond that.
    let receiver_iface = iface_name.clone();
    let receiver_handle = std::thread::spawn(move || {
        run_peer(
            &[
                "--role",
                "receiver",
                "--iface",
                &receiver_iface,
                "--count",
                &N.to_string(),
                "--recv-timeout-secs",
                "20",
            ],
            Duration::from_secs(30),
        )
    });
    std::thread::sleep(Duration::from_millis(300));

    let cangen_status = Command::new("cangen")
        .args([iface_name.as_str(), "-g", "5", "-n", &N.to_string()])
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn cangen: {e}"));
    assert!(
        cangen_status.success(),
        "cangen exited non-zero: {cangen_status:?}"
    );

    let receiver = receiver_handle.join().expect("receiver thread panicked");
    let mut candump_frames = capture.stop_and_parse();

    assert!(
        receiver.ok,
        "rust-CAN receiver did not report success (cangen may not have sent {N} frames in \
         time): {receiver:?}"
    );
    let mut received = receiver
        .received
        .expect("receiver report missing `received`");

    assert_eq!(
        candump_frames.len(),
        received.len(),
        "candump's independent capture and rust-CAN's receiver saw a different number of \
         frames from cangen — candump={candump_frames:?} received={received:?}"
    );

    // Compare as multisets (sorted): both sides observe the same real
    // broadcast stream from cangen, so content must match exactly, but
    // this test's job is decode-correctness, not delivery-ordering
    // fidelity across two independent listening sockets.
    candump_frames.sort();
    received.sort();
    assert_eq!(
        candump_frames, received,
        "rust-CAN's SocketCAN receiver decoded frames that don't field-exact match candump's \
         independent capture of what cangen actually put on the wire"
    );
}

// ---------------------------------------------------------------------------
// Deliverable 2/2, test 2: rust-CAN sender -> candump
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-001
//fusa:test REQ-CAN-010
#[test]
#[ignore = "requires a real vcan0 interface and can-utils installed; run via the can-interop CI job"]
fn rust_can_sender_frames_are_captured_field_exact_by_candump() {
    if skip_if_environment_unavailable() {
        return;
    }
    let iface_name = iface();
    const N: usize = 10;

    let capture = CandumpCapture::start(&iface_name);

    let sender = run_peer(
        &[
            "--role",
            "sender",
            "--iface",
            &iface_name,
            "--id",
            "285212672", // 0x11000000 — arbitrary extended (>0x7FF) value; see assert below
            "--ext",
            "--count",
            &N.to_string(),
            "--seed",
            "99",
            "--interval-ms",
            "10",
        ],
        Duration::from_secs(30),
    );

    let mut candump_frames = capture.stop_and_parse();

    assert!(sender.ok, "sender did not report success: {sender:?}");
    let mut sent = sender.sent.expect("sender report missing `sent`");
    assert_eq!(sent.len(), N);

    assert_eq!(
        candump_frames.len(),
        sent.len(),
        "candump captured a different number of frames than can-interop-peer reported \
         sending — candump={candump_frames:?} sent={sent:?}"
    );

    candump_frames.sort();
    sent.sort();
    assert_eq!(
        candump_frames, sent,
        "candump's independent capture of the real vcan0 wire traffic doesn't field-exact \
         match what can-interop-peer's SocketCAN sender reported transmitting"
    );
    for f in &candump_frames {
        assert_eq!(f.id, 285_212_672);
        assert!(f.ext);
    }
}
