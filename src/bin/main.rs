// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! rust-can CLI binary — RELAY spec §11 conformant command surface.

use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;

use rust_can::relay::{Context, Protocol, SubscriberOptions};
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
    Send {
        /// Network interface name (informational — uses virtual bus).
        #[arg(long)]
        iface: String,
        /// CAN frame ID (decimal or hex with 0x prefix).
        #[arg(long)]
        id: String,
        /// Frame data as hex string (e.g. DEADBEEF).
        #[arg(long)]
        data: String,
        /// Send as CAN FD frame.
        #[arg(long)]
        fd: bool,
        /// Use extended (29-bit) frame ID.
        #[arg(long)]
        ext: bool,
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
        } => cmd_send(iface, id, data, fd, ext).await,
        Commands::Subscribe {
            iface,
            count,
            format,
        } => cmd_subscribe(iface, count, format).await,
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
        "commands":            ["version", "capabilities", "status", "send", "subscribe"],
        "transports":          transports,
        "features":            ["fd", "isotp", "j1939", "safety", "dbc"],
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

/// `rust-can send --iface <name> --id <uint> --data <hex> [--fd] [--ext]`
async fn cmd_send(
    _iface: String,
    id_str: String,
    data_hex: String,
    fd: bool,
    ext: bool,
) -> Result<i32, Box<dyn std::error::Error>> {
    // Parse ID (decimal or 0x-prefixed hex).
    let id: u32 = if id_str.starts_with("0x") || id_str.starts_with("0X") {
        u32::from_str_radix(&id_str[2..], 16)?
    } else {
        id_str.parse()?
    };

    // Parse data hex.
    let data = hex::decode(data_hex.replace(' ', ""))?;

    let frame = Frame {
        id,
        ext,
        fd,
        data,
        ..Default::default()
    };

    // Validate the frame before sending.
    rust_can::validate_frame(&frame)?;

    // Use virtual bus for the send command.
    let bus = Arc::new(VirtualBus::new());
    bus.send(Context::background(), frame.clone()).await?;

    println!(
        "sent: id=0x{:X} ext={} fd={} data={}",
        id,
        ext,
        fd,
        hex::encode(&frame.data)
    );

    Ok(0)
}

// ---------------------------------------------------------------------------
// subscribe
// ---------------------------------------------------------------------------

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
                            "rtr":      frame.rtr,
                            "seq":      received,
                        });
                        println!("{}", serde_json::to_string(&doc)?);
                    }
                    OutputFormat::Text => {
                        println!(
                            "[{}] id=0x{:X} ext={} fd={} data={}",
                            received,
                            frame.id,
                            frame.ext,
                            frame.fd,
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
