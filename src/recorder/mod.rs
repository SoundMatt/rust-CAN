// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Candump-compatible CAN frame recording and replay.
//!
//! `record()` captures frames from a Bus to an `AsyncWrite` in candump format.
//! `replay()` reads a candump log and re-sends frames to a Bus, preserving timing.
//!
//! Candump format (one frame per line):
//! ```text
//! (timestamp) iface can_id#hexdata
//! (timestamp) iface can_id##flagshexdata   (CAN FD)
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::bus::Bus;
use crate::error::Error;
use crate::frame::Frame;
use crate::relay::{Context, SubscriberOptions};

// ---------------------------------------------------------------------------
// Record
// ---------------------------------------------------------------------------

/// Record all frames from `bus` to `w` in candump format.
///
/// Writes one line per frame until the context token is cancelled or the bus
/// subscription closes. `iface` is the interface label written to each line.
//fusa:req REQ-REC-001
pub async fn record(
    bus: Arc<dyn Bus>,
    w: &mut (impl Write + Send),
    iface: &str,
) -> Result<(), Error> {
    let rx = bus.subscribe(vec![], SubscriberOptions::default()).await?;

    loop {
        match rx.recv().await {
            None => return Ok(()),
            Some(f) => {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO);
                let line = format_line(iface, ts, &f);
                writeln!(w, "{}", line).map_err(|e| Error::Other(e.to_string()))?;
            }
        }
    }
}

/// Replay a candump log from `r`, sending each frame to `bus`.
///
/// Preserves inter-frame timing scaled by `speed_factor` (1.0 = real-time).
/// Malformed lines are skipped silently.
//fusa:req REQ-REC-002
pub async fn replay(
    bus: Arc<dyn Bus>,
    r: impl Read + Send,
    speed_factor: f64,
) -> Result<(), Error> {
    let speed_factor = if speed_factor <= 0.0 {
        1.0
    } else {
        speed_factor
    };

    let reader = BufReader::new(r);
    let mut log_t0: Option<Duration> = None;
    let wall0 = tokio::time::Instant::now();

    for line in reader.lines() {
        let line = line.map_err(|e| Error::Other(e.to_string()))?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (ts, frame) = match parse_line(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        match log_t0 {
            None => {
                log_t0 = Some(ts);
            }
            Some(t0) => {
                let log_delay = ts.saturating_sub(t0);
                let scaled_ns = (log_delay.as_nanos() as f64 / speed_factor) as u64;
                let scaled = Duration::from_nanos(scaled_ns);
                let elapsed = wall0.elapsed();
                if scaled > elapsed {
                    tokio::time::sleep(scaled - elapsed).await;
                }
            }
        }

        bus.send(Context::background(), frame).await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Line format
// ---------------------------------------------------------------------------

/// Parse a single candump line into `(timestamp, Frame)`.
///
/// Supports:
/// - `(TS) iface ID#HEXDATA`           — classic CAN
/// - `(TS) iface ID##FLAGSHEXDATA`     — CAN FD
//fusa:req REQ-REC-003
pub fn parse_line(line: &str) -> Result<(Duration, Frame), Error> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(Error::Other(format!(
            "recorder: expected 3 fields in {:?}",
            line
        )));
    }

    // Parse timestamp: (SECONDS.MICROS)
    let ts_raw = parts[0];
    if !ts_raw.starts_with('(') || !ts_raw.ends_with(')') {
        return Err(Error::Other(format!(
            "recorder: bad timestamp {:?}",
            ts_raw
        )));
    }
    let ts_str = &ts_raw[1..ts_raw.len() - 1];
    let ts = parse_timestamp(ts_str)?;

    let frame_str = parts[2];
    let frame = parse_frame_str(frame_str)?;

    Ok((ts, frame))
}

/// Format a frame as a candump line.
//fusa:req REQ-REC-003
pub fn format_line(iface: &str, ts: Duration, f: &Frame) -> String {
    let secs = ts.as_secs();
    let micros = ts.subsec_micros();
    let ts_str = format!("({}.{:06})", secs, micros);
    let id_str = format!("{:X}", f.id);

    if f.fd {
        let mut flags: u8 = 0;
        if f.brs {
            flags |= 0x01;
        }
        format!(
            "{} {} {}##{:02X}{}",
            ts_str,
            iface,
            id_str,
            flags,
            hex::encode_upper(&f.data)
        )
    } else {
        format!(
            "{} {} {}#{}",
            ts_str,
            iface,
            id_str,
            hex::encode_upper(&f.data)
        )
    }
}

fn parse_timestamp(s: &str) -> Result<Duration, Error> {
    let bad = || Error::Other(format!("recorder: invalid timestamp {:?}", s));
    match s.find('.') {
        None => {
            let secs: u64 = s.parse().map_err(|_| bad())?;
            Ok(Duration::from_secs(secs))
        }
        Some(dot) => {
            let secs: u64 = s[..dot].parse().map_err(|_| bad())?;
            let frac_str = &s[dot + 1..];
            let mut frac_str = frac_str.to_string();
            while frac_str.len() < 6 {
                frac_str.push('0');
            }
            frac_str.truncate(6);
            let usecs: u64 = frac_str.parse().map_err(|_| bad())?;
            Ok(Duration::from_secs(secs) + Duration::from_micros(usecs))
        }
    }
}

fn parse_frame_str(s: &str) -> Result<Frame, Error> {
    let bad = |msg: &str| Error::Other(format!("recorder: {}", msg));

    if let Some(idx) = s.find("##") {
        // CAN FD
        let id = u32::from_str_radix(&s[..idx], 16)
            .map_err(|_| bad(&format!("invalid CAN ID in {:?}", s)))?;
        let rest = &s[idx + 2..];
        if rest.len() < 2 {
            return Err(bad("FD frame missing flags byte"));
        }
        let flags_bytes = hex::decode(&rest[..2]).map_err(|_| bad("invalid FD flags byte"))?;
        let flags = flags_bytes[0];
        let brs = flags & 0x01 != 0;
        let data = if rest.len() > 2 {
            hex::decode(&rest[2..]).map_err(|_| bad("invalid FD data"))?
        } else {
            vec![]
        };
        Ok(Frame {
            id,
            fd: true,
            brs,
            data,
            ..Default::default()
        })
    } else if let Some(idx) = s.find('#') {
        let id = u32::from_str_radix(&s[..idx], 16)
            .map_err(|_| bad(&format!("invalid CAN ID in {:?}", s)))?;
        let ext = id > 0x7FF;
        let data_str = &s[idx + 1..];
        let data = if data_str.is_empty() {
            vec![]
        } else {
            hex::decode(data_str).map_err(|_| bad("invalid data hex"))?
        };
        Ok(Frame {
            id,
            ext,
            data,
            ..Default::default()
        })
    } else {
        Err(bad(&format!("missing '#' in frame field {:?}", s)))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    //fusa:test REQ-REC-003
    #[test]
    fn format_and_parse_classic_frame() {
        let frame = Frame {
            id: 0x123,
            data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            ..Default::default()
        };
        let ts = Duration::from_micros(1_609_459_200_000_000);
        let line = format_line("vcan0", ts, &frame);
        assert!(line.contains("(1609459200.000000)"), "line: {}", line);
        assert!(line.contains("123#DEADBEEF"), "line: {}", line);

        let (parsed_ts, parsed_frame) = parse_line(&line).unwrap();
        assert_eq!(parsed_ts, ts);
        assert_eq!(parsed_frame.id, frame.id);
        assert_eq!(parsed_frame.data, frame.data);
    }

    //fusa:test REQ-REC-003
    #[test]
    fn format_and_parse_fd_frame() {
        let frame = Frame {
            id: 0x456,
            fd: true,
            brs: true,
            data: vec![0x01, 0x02, 0x03],
            ..Default::default()
        };
        let ts = Duration::from_micros(1_609_459_200_050_000);
        let line = format_line("vcan0", ts, &frame);
        assert!(line.contains("##"), "FD line must use ##: {}", line);

        let (_, parsed_frame) = parse_line(&line).unwrap();
        assert!(parsed_frame.fd);
        assert!(parsed_frame.brs);
        assert_eq!(parsed_frame.data, frame.data);
    }

    //fusa:test REQ-REC-003
    #[test]
    fn parse_extended_id_sets_ext_flag() {
        let line = "(1234567890.000000) vcan0 1FFFFFFF#DEADBEEF";
        let (_, frame) = parse_line(line).unwrap();
        assert!(frame.ext, "extended ID must set ext flag");
        assert_eq!(frame.id, 0x1FFF_FFFF);
    }

    //fusa:test REQ-REC-003
    #[test]
    fn malformed_line_returns_error() {
        assert!(parse_line("not a valid line").is_err());
        assert!(parse_line("(bad) vcan0 123#DEAD").is_err());
    }
}
