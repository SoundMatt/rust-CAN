// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! `can-interop-peer` — a standalone sender/receiver process, driven
//! entirely by the real, production `rust_can::socketcan::SocketCanBus`
//! (real `AF_CAN`/`CAN_RAW` kernel socket, real `write(2)`/`read(2)` frame
//! I/O, real `Bus::send`/`Bus::subscribe`, no test-only shortcuts).
//!
//! This is the "two real rust-CAN processes ... bound to the same real
//! `vcan0` SocketCAN interface" deliverable of RELAY interop testing (see
//! `ROADMAP.md`'s "Interop testing" section), mirroring rust-DDS's own
//! `rtps-interop-peer` (`rtps_interop_peer.rs`) as closely as CAN's very
//! different transport model allows: SocketCAN has no discovery handshake
//! (no SPDP/SEDP — a raw `AF_CAN` socket simply starts receiving whatever
//! frames the kernel broadcasts on the bound interface from the moment it
//! is bound), so this binary has no discovery phase, only a `--settle-ms`
//! delay before the sender's first write (the harness spawning two of
//! these still starts the receiver process first and gives it a moment to
//! open+bind its socket, for the same reason DDS's writer waits out a
//! settle delay after its own SEDP match: two independent OS processes
//! starting up is not atomic).
//!
//! Reused, unmodified, by both interop suites in this crate:
//! `tests/can_two_process_interop.rs` (deliverable 1: two of these talking
//! to each other over real `vcan0`) and `tests/can_thirdparty_interop.rs`
//! (deliverable 2: one of these talking to a live, independent
//! `can-utils` `cangen`/`candump` process on the same real interface) —
//! exactly the DDS precedent set by `cyclone_interop.rs` reusing
//! `rtps-interop-peer` rather than writing a second, parallel peer binary,
//! so both suites stay provably testing the same code path.
//!
//! Not part of the crate's public library API (this is a `[[bin]]` target)
//! and not wired into the `rust-can` CLI binary — purely a test/dev
//! support tool. Linux-only, like `rust_can::socketcan` itself: on any
//! other target this binary's `main` prints a note and exits `1` rather
//! than failing to compile (`cargo build`/`cargo clippy --all-targets`
//! still succeed on the non-Linux legs of this crate's OS matrix).
//!
//! On completion, prints exactly one line of JSON to stdout (always the
//! *last* line — earlier lines, if any, are human-readable progress notes
//! on stderr) describing what happened, and exits `0` on success, `1`
//! otherwise. See `Report` (in the Linux implementation module below) for
//! the exact shape.
//!
//! Zero `unsafe`: all real `AF_CAN` socket work is already encapsulated,
//! with explicit `//fusa:unsafe SAFETY:` annotations, inside
//! `rust_can::socketcan::SocketCanBus` (REQ-SEC-008); this binary only
//! calls that existing, safe `Bus` API.

#[cfg(target_os = "linux")]
mod linux_impl {
    use std::io::Write as _;
    use std::sync::Arc;
    use std::time::Duration;

    use clap::{Parser, ValueEnum};
    use serde::Serialize;

    use rust_can::relay::{Context, SubscriberOptions};
    use rust_can::socketcan::SocketCanBus;
    use rust_can::{Bus, Frame};

    #[derive(Clone, Copy, Debug, ValueEnum)]
    pub enum Role {
        Sender,
        Receiver,
    }

    #[derive(Parser)]
    #[command(
        name = "can-interop-peer",
        about = "Live SocketCAN interop test peer process (test/dev tool, not a public API)"
    )]
    pub struct Cli {
        #[arg(long, value_enum)]
        role: Role,
        /// Real SocketCAN network interface, e.g. `vcan0`.
        #[arg(long, default_value = "vcan0")]
        iface: String,
        /// Sender only: arbitration ID for every frame in this run
        /// (decimal or `0x`-prefixed hex).
        #[arg(long, default_value_t = 0x123)]
        id: u32,
        /// Sender only: extended (29-bit) frame format.
        #[arg(long, default_value_t = false)]
        ext: bool,
        /// Sender only: CAN FD frames instead of classic CAN.
        #[arg(long, default_value_t = false)]
        fd: bool,
        /// Sender: number of frames to send. Receiver: number of frames to
        /// wait for.
        #[arg(long, default_value_t = 5)]
        count: usize,
        /// Sender only: seed mixed into the deterministic per-frame data
        /// pattern (see `build_frame`), so two sender runs on the same
        /// topic/ID can be told apart if their reports are ever compared.
        #[arg(long, default_value_t = 0)]
        seed: u8,
        #[arg(long, default_value_t = 20)]
        interval_ms: u64,
        /// Sender only: delay after opening its own socket, before the
        /// first send — gives a concurrently-starting receiver process
        /// time to open+bind its own socket first. SocketCAN has no
        /// discovery handshake to wait out (unlike RTPS's SPDP/SEDP): a
        /// raw AF_CAN socket receives whatever the kernel broadcasts on
        /// the interface from the instant it is bound, and nothing sent
        /// before that instant. This is belt-and-suspenders on top of the
        /// test harness's own head start for the receiver process (see
        /// `tests/can_two_process_interop.rs::run_sender_and_receiver`).
        #[arg(long, default_value_t = 150)]
        settle_ms: u64,
        #[arg(long, default_value_t = 15)]
        recv_timeout_secs: u64,
    }

    #[derive(Debug, Clone, Serialize)]
    struct FrameRecord {
        id: u32,
        ext: bool,
        rtr: bool,
        fd: bool,
        brs: bool,
        esi: bool,
        dlc: usize,
        data_hex: String,
    }

    impl FrameRecord {
        fn from_frame(f: &Frame) -> Self {
            Self {
                id: f.id,
                ext: f.ext,
                rtr: f.rtr,
                fd: f.fd,
                brs: f.brs,
                esi: f.esi,
                dlc: f.data.len(),
                data_hex: hex::encode(&f.data),
            }
        }
    }

    #[derive(Serialize)]
    struct Report {
        role: &'static str,
        ok: bool,
        iface: String,
        count: usize,
        sent: Option<Vec<FrameRecord>>,
        received: Option<Vec<FrameRecord>>,
        error: Option<String>,
    }

    fn log(msg: impl AsRef<str>) {
        eprintln!("{}", msg.as_ref());
    }

    fn role_str(role: Role) -> &'static str {
        match role {
            Role::Sender => "sender",
            Role::Receiver => "receiver",
        }
    }

    fn print_report_and_exit_code(report: Report) -> i32 {
        let code = if report.ok { 0 } else { 1 };
        let json = serde_json::to_string(&report).unwrap_or_else(|e| {
            format!(r#"{{"ok":false,"error":"failed to serialise report: {e}"}}"#)
        });
        println!("{json}");
        let _ = std::io::stdout().flush();
        code
    }

    fn fail_report(role: Role, iface: &str, count: usize, error: impl Into<String>) -> i32 {
        let error = error.into();
        log(format!("can-interop-peer: FAIL: {error}"));
        print_report_and_exit_code(Report {
            role: role_str(role),
            ok: false,
            iface: iface.to_string(),
            count,
            sent: None,
            received: None,
            error: Some(error),
        })
    }

    /// Builds frame `i` of `count` deterministically from `cli`, cycling
    /// the data length across the frame's legal DLC range (1..=8 for
    /// classic, 1..=64 for FD) so a single run also proves DLC-exactness,
    /// and — when `--fd` is set — alternating BRS/ESI across frames so
    /// both FD-only flags round-trip through at least one frame each.
    fn build_frame(cli: &Cli, i: usize) -> Frame {
        let max_len = if cli.fd { 64 } else { 8 };
        let len = 1 + (i % max_len);
        let mut data = vec![0u8; len];
        for (j, b) in data.iter_mut().enumerate() {
            *b = cli
                .seed
                .wrapping_add((i as u8).wrapping_mul(31))
                .wrapping_add((j as u8).wrapping_mul(7))
                .wrapping_add(13);
        }
        Frame {
            id: cli.id,
            ext: cli.ext,
            fd: cli.fd,
            brs: cli.fd && i % 2 == 0,
            esi: cli.fd && i % 3 == 0,
            data,
            ..Default::default()
        }
    }

    async fn run_sender(cli: &Cli) -> i32 {
        let bus = match SocketCanBus::new(&cli.iface) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                return fail_report(
                    cli.role,
                    &cli.iface,
                    cli.count,
                    format!("open SocketCAN iface {:?}: {e}", cli.iface),
                )
            }
        };

        if cli.settle_ms > 0 {
            tokio::time::sleep(Duration::from_millis(cli.settle_ms)).await;
        }

        log(format!(
            "can-interop-peer: sender on {:?} sending {} frame(s), id=0x{:X} ext={} fd={}",
            cli.iface, cli.count, cli.id, cli.ext, cli.fd
        ));

        let mut sent = Vec::with_capacity(cli.count);
        for i in 0..cli.count {
            let frame = build_frame(cli, i);
            if let Err(e) = bus.send(Context::background(), frame.clone()).await {
                let mut report_sent = sent.clone();
                report_sent.push(FrameRecord::from_frame(&frame));
                log(format!("can-interop-peer: FAIL: send {i} failed: {e}"));
                return print_report_and_exit_code(Report {
                    role: "sender",
                    ok: false,
                    iface: cli.iface.clone(),
                    count: cli.count,
                    sent: Some(report_sent),
                    received: None,
                    error: Some(format!("send {i} failed: {e}")),
                });
            }
            sent.push(FrameRecord::from_frame(&frame));
            if cli.interval_ms > 0 {
                tokio::time::sleep(Duration::from_millis(cli.interval_ms)).await;
            }
        }

        let _ = bus.close().await;

        print_report_and_exit_code(Report {
            role: "sender",
            ok: true,
            iface: cli.iface.clone(),
            count: cli.count,
            sent: Some(sent),
            received: None,
            error: None,
        })
    }

    async fn run_receiver(cli: &Cli) -> i32 {
        let bus = match SocketCanBus::new(&cli.iface) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                return fail_report(
                    cli.role,
                    &cli.iface,
                    cli.count,
                    format!("open SocketCAN iface {:?}: {e}", cli.iface),
                )
            }
        };
        let rx = match bus.subscribe(vec![], SubscriberOptions::default()).await {
            Ok(rx) => rx,
            Err(e) => {
                return fail_report(cli.role, &cli.iface, cli.count, format!("subscribe: {e}"))
            }
        };

        log(format!(
            "can-interop-peer: receiver on {:?} waiting for {} frame(s)",
            cli.iface, cli.count
        ));

        let mut received = Vec::with_capacity(cli.count);
        let collect = tokio::time::timeout(Duration::from_secs(cli.recv_timeout_secs), async {
            while received.len() < cli.count {
                match rx.recv().await {
                    Some(f) => received.push(FrameRecord::from_frame(&f)),
                    None => break,
                }
            }
        })
        .await;
        if collect.is_err() {
            log(format!(
                "can-interop-peer: recv timeout after {} frame(s) (wanted {})",
                received.len(),
                cli.count
            ));
        }

        let ok = received.len() == cli.count;
        let error = if ok {
            None
        } else {
            Some(format!(
                "received {} of {} expected frame(s) within {}s",
                received.len(),
                cli.count,
                cli.recv_timeout_secs
            ))
        };
        print_report_and_exit_code(Report {
            role: "receiver",
            ok,
            iface: cli.iface.clone(),
            count: cli.count,
            sent: None,
            received: Some(received),
            error,
        })
    }

    pub async fn main_impl() -> i32 {
        let cli = Cli::parse();
        match cli.role {
            Role::Sender => run_sender(&cli).await,
            Role::Receiver => run_receiver(&cli).await,
        }
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    {
        let rt = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
        let code = rt.block_on(linux_impl::main_impl());
        std::process::exit(code);
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!(
            "can-interop-peer: rust_can::socketcan is Linux-only; nothing to do on this platform"
        );
        std::process::exit(1);
    }
}
