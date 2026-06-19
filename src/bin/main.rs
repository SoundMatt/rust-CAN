// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! rust-can CLI binary — RELAY spec §11 conformant command surface.

use std::io::BufRead;
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use rust_can::relay::{Context, Message, Protocol, SubscriberOptions};
use rust_can::virtual_bus::VirtualBus;
use rust_can::{Bus, Frame};

// ---------------------------------------------------------------------------
// CLI argument definitions
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "rust-can",
    version = env!("CARGO_PKG_VERSION"),
    about = "rust-CAN: RELAY-conformant CAN bus tool"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Report tool and protocol version.
    Version {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Report supported capabilities as JSON.
    Capabilities,

    /// Report self-assessed health status.
    Status {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Send a CAN frame via the virtual bus.
    ///
    /// Use --format json to read newline-delimited relay.Message JSON from stdin
    /// (crossbar destination mode, RELAY v1.8 §send). In that mode --id and
    /// --data are ignored.
    Send {
        /// Network interface name (informational — uses virtual bus).
        #[arg(long)]
        iface: String,
        /// CAN frame ID (decimal or hex with 0x prefix). Optional when --format json.
        #[arg(long)]
        id: Option<String>,
        /// Frame data as hex string (e.g. DEADBEEF). Optional when --format json.
        #[arg(long)]
        data: Option<String>,
        /// Send as CAN FD frame.
        #[arg(long)]
        fd: bool,
        /// Use extended (29-bit) frame ID.
        #[arg(long)]
        ext: bool,
        /// Send as CAN XL frame (mutually exclusive with --fd).
        #[arg(long)]
        xl: bool,
        /// CAN XL SDU Type (0–255, XL only).
        #[arg(long, default_value = "0")]
        sdt: u8,
        /// CAN XL Virtual CAN network ID (0–255, XL only).
        #[arg(long, default_value = "0")]
        vcid: u8,
        /// CAN XL Acceptance Field (XL only).
        #[arg(long, default_value = "0")]
        af: u32,
        /// CAN XL Simple Extended Content flag (XL only).
        #[arg(long)]
        sec: bool,
        /// Input format: 'text' (default, uses --id/--data) or 'json' (reads
        /// NDJSON relay.Message from stdin).
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Subscribe to CAN frames on the virtual bus.
    Subscribe {
        /// Network interface name (informational — uses virtual bus).
        #[arg(long)]
        iface: String,
        /// Stop after receiving N frames (0 = unlimited).
        #[arg(long, default_value = "0")]
        count: usize,
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Convert a CAN frame JSON from stdin to relay.Message JSON on stdout.
    ///
    /// Reads one can.Frame as JSON on stdin, converts it through this
    /// implementation's to_message() path, and writes the relay.Message
    /// JSON on stdout. Used by `relay interop` (RELAY spec §11.2).
    ///
    /// Exit codes: 0 = converted, 1 = invalid input, 2 = invalid args.
    Convert {
        /// Protocol identifier; must be CAN for this tool.
        #[arg(long, default_value = "CAN")]
        protocol: String,
        /// Output format.
        #[arg(long, value_enum, default_value = "json")]
        format: OutputFormat,
    },
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let exit_code = match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rust-can: error: {}", e);
            1
        }
    };

    std::process::exit(exit_code);
}

async fn run(cli: Cli) -> Result<i32, Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Version { format } => cmd_version(format),
        Commands::Capabilities => cmd_capabilities(),
        Commands::Status { format } => cmd_status(format),
        Commands::Send {
            iface,
            id,
            data,
            fd,
            ext,
            xl,
            sdt,
            vcid,
            af,
            sec,
            format,
        } => cmd_send(iface, id, data, fd, ext, xl, sdt, vcid, af, sec, format).await,
        Commands::Subscribe {
            iface,
            count,
            format,
        } => cmd_subscribe(iface, count, format).await,
        Commands::Convert { protocol, format } => cmd_convert(protocol, format),
    }
}

// ---------------------------------------------------------------------------
// version
// ---------------------------------------------------------------------------

/// `rust-can version [--format text|json]` — RELAY spec §11.1 / §12.1
fn cmd_version(format: OutputFormat) -> Result<i32, Box<dyn std::error::Error>> {
    let doc = json!({
        "tool":         "rust-can",
        "protocol":     "CAN",
        "protocol_int": Protocol::Can as i32,
        "version":      env!("CARGO_PKG_VERSION"),
        "spec_version": rust_can::SPEC_VERSION,
        "language":     "rust",
        "runtime":      "rustc 1.75+",
    });

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&doc)?),
        OutputFormat::Text => {
            println!("tool:         {}", doc["tool"].as_str().unwrap_or(""));
            println!("protocol:     {}", doc["protocol"].as_str().unwrap_or(""));
            println!("version:      {}", doc["version"].as_str().unwrap_or(""));
            println!(
                "spec_version: {}",
                doc["spec_version"].as_str().unwrap_or("")
            );
            println!("language:     {}", doc["language"].as_str().unwrap_or(""));
            println!("runtime:      {}", doc["runtime"].as_str().unwrap_or(""));
        }
    }

    Ok(0)
}

// ---------------------------------------------------------------------------
// capabilities
// ---------------------------------------------------------------------------

/// `rust-can capabilities` — RELAY spec §11.1 / §12.2
fn cmd_capabilities() -> Result<i32, Box<dyn std::error::Error>> {
    // Transports: virtual always present; socketcan only on Linux.
    let transports = {
        #[cfg(target_os = "linux")]
        {
            vec!["virtual", "socketcan"]
        }
        #[cfg(not(target_os = "linux"))]
        {
            vec!["virtual"]
        }
    };

    let doc = json!({
        "kind":                "capabilities",
        "tool":                "rust-can",
        "protocol":            "CAN",
        "protocol_int":        Protocol::Can as i32,
        "version":             env!("CARGO_PKG_VERSION"),
        "spec_version":        rust_can::SPEC_VERSION,
        "commands":            ["version", "capabilities", "status", "send", "subscribe", "convert"],
        "transports":          transports,
        "features":            ["fd", "xl", "isotp", "j1939", "safety", "dbc", "recorder", "obdii", "uds"],
        "interfaces":          ["Bus"],
        "optional_interfaces": ["LoaningBus", "HealthProvider", "MetricsProvider", "Drainer"],
        "adapt":               true,
    });

    println!("{}", serde_json::to_string_pretty(&doc)?);
    Ok(0)
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

/// `rust-can status [--format text|json]` — RELAY spec §11.1 / §12.3
fn cmd_status(format: OutputFormat) -> Result<i32, Box<dyn std::error::Error>> {
    let doc = json!({
        "protocol":  "CAN",
        "tool":      "rust-can",
        "version":   env!("CARGO_PKG_VERSION"),
        "healthy":   true,
        "connected": false,
        "endpoint":  "",
        "details":   {},
    });

    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&doc)?),
        OutputFormat::Text => {
            println!("tool:      {}", doc["tool"].as_str().unwrap_or(""));
            println!("protocol:  {}", doc["protocol"].as_str().unwrap_or(""));
            println!("version:   {}", doc["version"].as_str().unwrap_or(""));
            println!("healthy:   {}", doc["healthy"]);
            println!("connected: {}", doc["connected"]);
        }
    }

    Ok(0)
}

// ---------------------------------------------------------------------------
// send
// ---------------------------------------------------------------------------

//fusa:req REQ-CAN-003
//fusa:req REQ-CAN-004
//fusa:req REQ-SEND-001
/// `rust-can send --iface <name> [--id <uint>] [--data <hex>] [--format json|text]`
///
/// In text mode (default) sends a single frame built from --id/--data flags.
/// In json mode reads NDJSON relay.Message objects from stdin, converts each
/// via from_message(), and sends the resulting frame to the virtual bus.
#[allow(clippy::too_many_arguments)]
async fn cmd_send(
    _iface: String,
    id_str: Option<String>,
    data_hex: Option<String>,
    fd: bool,
    ext: bool,
    xl: bool,
    sdt: u8,
    vcid: u8,
    af: u32,
    sec: bool,
    format: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    let bus = Arc::new(VirtualBus::new());

    match format {
        OutputFormat::Json => {
            // Streaming JSON: read NDJSON relay.Message from stdin.
            cmd_send_json(bus).await
        }
        OutputFormat::Text => {
            // Classic single-frame send from flags.
            let id_str = id_str.ok_or("rust-can: --id is required in text mode")?;
            let data_hex = data_hex.ok_or("rust-can: --data is required in text mode")?;

            let id: u32 = if id_str.starts_with("0x") || id_str.starts_with("0X") {
                u32::from_str_radix(&id_str[2..], 16)?
            } else {
                id_str.parse()?
            };

            let data = hex::decode(data_hex.replace(' ', ""))?;

            let frame = Frame {
                id,
                ext,
                fd,
                xl,
                sdt,
                vcid,
                af,
                sec,
                data,
                ..Default::default()
            };

            rust_can::validate_frame(&frame)?;
            bus.send(Context::background(), frame.clone()).await?;

            println!(
                "sent: id=0x{:X} ext={} fd={} xl={} data={}",
                id,
                ext,
                fd,
                xl,
                hex::encode(&frame.data)
            );

            Ok(0)
        }
    }
}

//fusa:req REQ-SEND-001
/// Read NDJSON relay.Message objects from stdin and send each as a CAN frame.
async fn cmd_send_json(bus: Arc<VirtualBus>) -> Result<i32, Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let mut sent = 0usize;
    let mut errors = 0usize;

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let msg: Message = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("rust-can: send --format json: parse error: {}", e);
                errors += 1;
                continue;
            }
        };

        let frame = match rust_can::from_message(&msg) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("rust-can: send --format json: conversion error: {}", e);
                errors += 1;
                continue;
            }
        };

        if let Err(e) = rust_can::validate_frame(&frame) {
            eprintln!("rust-can: send --format json: invalid frame: {}", e);
            errors += 1;
            continue;
        }

        bus.send(Context::background(), frame).await?;
        sent += 1;
    }

    eprintln!(
        "rust-can: send --format json: sent={} errors={}",
        sent, errors
    );
    Ok(0)
}

// ---------------------------------------------------------------------------
// subscribe
// ---------------------------------------------------------------------------

//fusa:req REQ-CAN-003
/// `rust-can subscribe --iface <name> [--count N] [--format text|json]`
async fn cmd_subscribe(
    _iface: String,
    count: usize,
    format: OutputFormat,
) -> Result<i32, Box<dyn std::error::Error>> {
    let bus = Arc::new(VirtualBus::new());
    let rx = bus.subscribe(vec![], SubscriberOptions::default()).await?;

    eprintln!(
        "rust-can: subscribing on virtual bus ({})",
        if count == 0 {
            "unlimited".to_string()
        } else {
            format!("{} frames", count)
        }
    );

    let mut received = 0usize;
    loop {
        if count > 0 && received >= count {
            break;
        }

        match rx.recv().await {
            None => break,
            Some(frame) => {
                received += 1;
                let msg = rust_can::to_message(&frame);

                match format {
                    OutputFormat::Json => {
                        let doc = json!({
                            "protocol": "CAN",
                            "id":       msg.id,
                            "data":     hex::encode(&frame.data),
                            "ext":      frame.ext,
                            "fd":       frame.fd,
                            "xl":       frame.xl,
                            "rtr":      frame.rtr,
                            "seq":      received,
                        });
                        println!("{}", serde_json::to_string(&doc)?);
                    }
                    OutputFormat::Text => {
                        println!(
                            "[{}] id=0x{:X} ext={} fd={} xl={} data={}",
                            received,
                            frame.id,
                            frame.ext,
                            frame.fd,
                            frame.xl,
                            hex::encode(&frame.data)
                        );
                    }
                }
            }
        }
    }

    bus.close().await?;
    Ok(0)
}

// ---------------------------------------------------------------------------
// convert  (RELAY spec §11.2)
// ---------------------------------------------------------------------------

//fusa:req REQ-CAN-007
//fusa:req REQ-CAN-015
//fusa:req REQ-SEC-001
/// `rust-can convert --protocol CAN [--format json]`
///
/// Reads a `can.Frame` JSON object from stdin, converts it through
/// `to_message()`, and writes the resulting `relay.Message` JSON on stdout.
///
/// Exit codes: 0 = converted, 1 = invalid input, 2 = invalid args.
fn cmd_convert(protocol: String, _format: OutputFormat) -> Result<i32, Box<dyn std::error::Error>> {
    if !protocol.eq_ignore_ascii_case("CAN") {
        eprintln!(
            "rust-can: convert: unsupported protocol '{}'; this tool implements CAN",
            protocol
        );
        // Exit 2 = invalid args per spec §11.2.
        return Ok(2);
    }

    // Frame derives Deserialize — parse stdin directly.
    let frame: Frame = match serde_json::from_reader(std::io::stdin().lock()) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{}", e);
            eprintln!("INVALID_ARGUMENT");
            return Ok(1);
        }
    };

    if let Err(e) = rust_can::validate_frame(&frame) {
        eprintln!("{}", e);
        eprintln!("INVALID_ARGUMENT");
        return Ok(1);
    }

    let mut msg = rust_can::to_message(&frame);
    // Zero the timestamp per spec §11.2: "timestamp may be zeroed".
    msg.timestamp = chrono::DateTime::UNIX_EPOCH;

    println!("{}", serde_json::to_string_pretty(&msg)?);
    Ok(0)
}
