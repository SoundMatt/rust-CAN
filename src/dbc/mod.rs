// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! DBC file parser and CAN signal decoder.
//!
//! Parses `BO_` message blocks and `SG_` signal definitions from DBC files,
//! then decodes signal values from raw CAN frame data.

use std::collections::HashMap;

use crate::error::Error;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Bit-layout order for a DBC signal.
//fusa:req REQ-DBC-001
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    /// Intel (little-endian): LSB at start_bit, bits extend upward.
    LittleEndian,
    /// Motorola (big-endian): MSB at start_bit, bits extend downward.
    BigEndian,
}

/// Whether a signal's raw integer value is signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Unsigned,
    Signed,
}

/// A single signal definition within a DBC message.
//fusa:req REQ-DBC-001
#[derive(Debug, Clone)]
pub struct DbcSignal {
    pub name: String,
    pub start_bit: u8,
    pub length: u8,
    pub byte_order: ByteOrder,
    pub value_type: ValueType,
    pub factor: f64,
    pub offset: f64,
    pub min: f64,
    pub max: f64,
    pub unit: String,
}

/// A message definition from a DBC file.
//fusa:req REQ-DBC-001
#[derive(Debug, Clone)]
pub struct DbcMessage {
    pub id: u32,
    pub name: String,
    pub dlc: u8,
    pub signals: Vec<DbcSignal>,
}

/// A parsed DBC database.
//fusa:req REQ-DBC-001
#[derive(Debug, Clone, Default)]
pub struct DbcDatabase {
    pub messages: HashMap<u32, DbcMessage>,
}

impl DbcDatabase {
    /// Decode all signals for the given message ID from raw frame data.
    ///
    /// Returns an empty map if the message ID is not known.
    //fusa:req REQ-DBC-002
    pub fn decode(&self, id: u32, data: &[u8]) -> HashMap<String, f64> {
        let msg = match self.messages.get(&id) {
            Some(m) => m,
            None => return HashMap::new(),
        };

        let mut result = HashMap::new();
        for sig in &msg.signals {
            let value = decode_signal(sig, data);
            result.insert(sig.name.clone(), value);
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse a DBC file from a string slice.
///
/// Recognises `BO_` (message) and `SG_` (signal) lines; all other content
/// is ignored. This is a line-by-line parser — not a full DBC grammar.
//fusa:req REQ-DBC-001
pub fn parse(input: &str) -> Result<DbcDatabase, Error> {
    let mut db = DbcDatabase::default();
    let mut current_msg: Option<DbcMessage> = None;

    for raw_line in input.lines() {
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with("//") {
            // If we were accumulating signals for a message and hit a blank
            // line that ends the message block, commit it.
            if !line.starts_with("SG_") {
                if let Some(msg) = current_msg.take() {
                    db.messages.insert(msg.id, msg);
                }
            }
            continue;
        }

        if line.starts_with("BO_ ") {
            // Commit any in-flight message.
            if let Some(msg) = current_msg.take() {
                db.messages.insert(msg.id, msg);
            }
            current_msg = Some(parse_message_line(line)?);
            continue;
        }

        if line.starts_with("SG_ ") {
            if let Some(ref mut msg) = current_msg {
                let sig = parse_signal_line(line)?;
                msg.signals.push(sig);
            }
            continue;
        }

        // Any other keyword ends the current message block.
        if let Some(msg) = current_msg.take() {
            db.messages.insert(msg.id, msg);
        }
    }

    // Commit the last in-flight message.
    if let Some(msg) = current_msg.take() {
        db.messages.insert(msg.id, msg);
    }

    Ok(db)
}

// ---------------------------------------------------------------------------
// Line parsers
// ---------------------------------------------------------------------------

/// Parse a `BO_` message definition line.
///
/// Format: `BO_ <id> <name>: <dlc> <sender>`
fn parse_message_line(line: &str) -> Result<DbcMessage, Error> {
    // BO_ 256 EngineData: 8 ECU
    let parts: Vec<&str> = line.splitn(5, ' ').collect();
    if parts.len() < 4 {
        return Err(Error::Other(format!("dbc: malformed BO_ line: '{}'", line)));
    }

    let id: u32 = parts[1]
        .parse()
        .map_err(|_| Error::Other(format!("dbc: invalid message ID: '{}'", parts[1])))?;

    let name = parts[2].trim_end_matches(':').to_string();

    let dlc: u8 = parts[3]
        .parse()
        .map_err(|_| Error::Other(format!("dbc: invalid DLC: '{}'", parts[3])))?;

    Ok(DbcMessage {
        id,
        name,
        dlc,
        signals: Vec::new(),
    })
}

/// Parse a `SG_` signal definition line.
///
/// Format: `SG_ <name> : <start_bit>|<length>@<byte_order><value_type> (<factor>,<offset>) [<min>|<max>] "<unit>" <receivers>`
fn parse_signal_line(line: &str) -> Result<DbcSignal, Error> {
    // SG_ EngineSpeed : 0|16@1+ (0.25,0) [0|16383.75] "rpm" Vector__XXX

    // Split on ':' to get name and rest.
    let colon_pos = line
        .find(':')
        .ok_or_else(|| Error::Other(format!("dbc: missing ':' in SG_ line: '{}'", line)))?;

    let name_part = line[4..colon_pos].trim().to_string();
    // Handle multiplexed signal indicator (e.g., " M " or " m1 ")
    let name = name_part
        .split_whitespace()
        .next()
        .unwrap_or(&name_part)
        .to_string();

    let rest = line[colon_pos + 1..].trim();

    // Parse: <start_bit>|<length>@<byte_order><value_type>
    let space_pos = rest
        .find(' ')
        .ok_or_else(|| Error::Other(format!("dbc: malformed SG_ layout: '{}'", rest)))?;

    let layout = &rest[..space_pos];
    let rest = rest[space_pos..].trim();

    let pipe_pos = layout
        .find('|')
        .ok_or_else(|| Error::Other("dbc: missing '|' in signal layout".into()))?;
    let at_pos = layout
        .find('@')
        .ok_or_else(|| Error::Other("dbc: missing '@' in signal layout".into()))?;

    let start_bit: u8 = layout[..pipe_pos]
        .parse()
        .map_err(|_| Error::Other(format!("dbc: invalid start bit: '{}'", &layout[..pipe_pos])))?;

    let len_and_flags = &layout[pipe_pos + 1..];
    let len_str = &len_and_flags[..at_pos - pipe_pos - 1];
    let length: u8 = len_str
        .parse()
        .map_err(|_| Error::Other(format!("dbc: invalid signal length: '{}'", len_str)))?;

    let flags = &layout[at_pos + 1..];
    let byte_order = match flags.chars().next() {
        Some('1') => ByteOrder::LittleEndian,
        Some('0') => ByteOrder::BigEndian,
        other => {
            return Err(Error::Other(format!(
                "dbc: unknown byte order: {:?}",
                other
            )))
        }
    };
    let value_type = match flags.chars().nth(1) {
        Some('+') => ValueType::Unsigned,
        Some('-') => ValueType::Signed,
        other => {
            return Err(Error::Other(format!(
                "dbc: unknown value type: {:?}",
                other
            )))
        }
    };

    // Parse: (<factor>,<offset>)
    let lparen = rest
        .find('(')
        .ok_or_else(|| Error::Other("dbc: missing '(' in signal".into()))?;
    let rparen = rest
        .find(')')
        .ok_or_else(|| Error::Other("dbc: missing ')' in signal".into()))?;

    let fo = &rest[lparen + 1..rparen];
    let comma_pos = fo
        .find(',')
        .ok_or_else(|| Error::Other("dbc: missing ',' in factor/offset".into()))?;

    let factor: f64 = fo[..comma_pos]
        .parse()
        .map_err(|_| Error::Other("dbc: invalid factor".into()))?;
    let offset: f64 = fo[comma_pos + 1..]
        .parse()
        .map_err(|_| Error::Other("dbc: invalid offset".into()))?;

    let rest = rest[rparen + 1..].trim();

    // Parse: [<min>|<max>]
    let lbrace = rest.find('[').unwrap_or(0);
    let rbrace = rest.find(']').unwrap_or(0);
    let (min, max) = if lbrace < rbrace {
        let mm = &rest[lbrace + 1..rbrace];
        let pipe = mm.find('|').unwrap_or(mm.len());
        let min: f64 = mm[..pipe].parse().unwrap_or(0.0);
        let max: f64 = if pipe < mm.len() {
            mm[pipe + 1..].parse().unwrap_or(0.0)
        } else {
            0.0
        };
        (min, max)
    } else {
        (0.0, 0.0)
    };

    let rest = if rbrace < rest.len() {
        rest[rbrace + 1..].trim()
    } else {
        ""
    };

    // Parse: "<unit>"
    let unit = if rest.starts_with('"') {
        let end_quote = rest[1..].find('"').map(|p| p + 1).unwrap_or(rest.len() - 1);
        rest[1..end_quote].to_string()
    } else {
        String::new()
    };

    Ok(DbcSignal {
        name,
        start_bit,
        length,
        byte_order,
        value_type,
        factor,
        offset,
        min,
        max,
        unit,
    })
}

// ---------------------------------------------------------------------------
// Signal decoding
// ---------------------------------------------------------------------------

/// Decode a signal's physical value from raw frame bytes.
//fusa:req REQ-DBC-002
pub fn decode_signal(sig: &DbcSignal, data: &[u8]) -> f64 {
    let raw = extract_raw(
        data,
        sig.start_bit as usize,
        sig.length as usize,
        sig.byte_order,
    );

    let phys = match sig.value_type {
        ValueType::Unsigned => raw as f64 * sig.factor + sig.offset,
        ValueType::Signed => {
            // Two's complement sign extension.
            let bits = sig.length as usize;
            let signed = if raw & (1 << (bits - 1)) != 0 {
                // Negative number — extend sign.
                raw as i64 - (1i64 << bits)
            } else {
                raw as i64
            };
            signed as f64 * sig.factor + sig.offset
        }
    };

    phys
}

/// Extract a raw unsigned integer from `data` at `start_bit` with `length` bits.
fn extract_raw(data: &[u8], start_bit: usize, length: usize, order: ByteOrder) -> u64 {
    let mut raw: u64 = 0;

    match order {
        ByteOrder::LittleEndian => {
            // Intel format: start_bit is the LSB position.
            for i in 0..length {
                let bit = start_bit + i;
                let byte_idx = bit / 8;
                let bit_idx = bit % 8;
                if byte_idx < data.len() && (data[byte_idx] >> bit_idx) & 1 == 1 {
                    raw |= 1u64 << i;
                }
            }
        }
        ByteOrder::BigEndian => {
            // Motorola format: start_bit is the MSB position.
            let mut byte_idx = start_bit / 8;
            let mut bit_idx = start_bit % 8;
            for i in 0..length {
                if byte_idx < data.len() && (data[byte_idx] >> bit_idx) & 1 == 1 {
                    raw |= 1u64 << (length - 1 - i);
                }
                if bit_idx == 0 {
                    bit_idx = 7;
                    if byte_idx < usize::MAX {
                        byte_idx += 1;
                    }
                } else {
                    bit_idx -= 1;
                }
            }
        }
    }

    raw
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_DBC: &str = r#"
BO_ 256 EngineData: 8 ECU
 SG_ EngineSpeed : 0|16@1+ (0.25,0) [0|16383.75] "rpm" Vector__XXX
 SG_ EngineTemp : 16|8@1- (1,-40) [-40|215] "degC" Vector__XXX

BO_ 512 TransmissionData: 4 TCU
 SG_ GearPosition : 0|4@1+ (1,0) [0|15] "" Vector__XXX
"#;

    //fusa:req REQ-DBC-001
    #[test]
    fn parse_messages() {
        let db = parse(SAMPLE_DBC).unwrap();
        assert_eq!(db.messages.len(), 2);

        let engine = db.messages.get(&256).unwrap();
        assert_eq!(engine.name, "EngineData");
        assert_eq!(engine.dlc, 8);
        assert_eq!(engine.signals.len(), 2);

        let trans = db.messages.get(&512).unwrap();
        assert_eq!(trans.name, "TransmissionData");
        assert_eq!(trans.signals.len(), 1);
    }

    //fusa:req REQ-DBC-002
    #[test]
    fn decode_unsigned_signal() {
        let db = parse(SAMPLE_DBC).unwrap();
        // EngineSpeed: bits 0..16 LE, factor=0.25, offset=0
        // data = [0x00, 0x10, ...] → raw = 0x1000 = 4096 → 4096 * 0.25 = 1024.0
        let data: Vec<u8> = vec![0x00, 0x10, 0, 0, 0, 0, 0, 0];
        let values = db.decode(256, &data);
        let speed = values["EngineSpeed"];
        assert!((speed - 1024.0).abs() < 0.01, "speed={}", speed);
    }

    #[test]
    fn decode_signed_signal() {
        let db = parse(SAMPLE_DBC).unwrap();
        // EngineTemp: bits 16..24 LE signed, factor=1, offset=-40
        // data byte 2 = 0x00 → raw = 0 → 0*1 + (-40) = -40.0
        let data: Vec<u8> = vec![0, 0, 0x00, 0, 0, 0, 0, 0];
        let values = db.decode(256, &data);
        let temp = values["EngineTemp"];
        assert!((temp - (-40.0)).abs() < 0.01, "temp={}", temp);
    }

    #[test]
    fn decode_unknown_message_returns_empty() {
        let db = parse(SAMPLE_DBC).unwrap();
        let values = db.decode(9999, &[]);
        assert!(values.is_empty());
    }

    #[test]
    fn extract_raw_little_endian() {
        // 16-bit value at bit 0 from bytes [0x34, 0x12]
        let data = [0x34u8, 0x12];
        let raw = extract_raw(&data, 0, 16, ByteOrder::LittleEndian);
        assert_eq!(raw, 0x1234);
    }
}
