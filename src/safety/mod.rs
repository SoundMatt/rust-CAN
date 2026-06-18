// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! End-to-end (E2E) safety protection for CAN payloads.
//!
//! Wire format (little-endian, 10-byte header followed by the original payload):
//!
//! ```text
//! Bytes  0–1   DataID (uint16 LE)
//! Bytes  2–3   SourceID (uint16 LE)
//! Bytes  4–7   SequenceCounter (uint32 LE)
//! Bytes  8–9   CRC-16/CCITT-FALSE over bytes 0–7 (CRC slot = 0) + payload
//! Bytes 10+    Original payload
//! ```

use std::sync::{
    atomic::{AtomicU32, Ordering},
    Mutex,
};

use crate::crc::crc16_ccitt_false;

/// Size of the E2E header in bytes.
const HEADER_SIZE: usize = 10;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for E2E protection.
//fusa:req REQ-SAFETY-001
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Logical data element identifier (0–65535).
    pub data_id: u16,
    /// Sender identifier (0–65535).
    pub source_id: u16,
}

// ---------------------------------------------------------------------------
// E2EErrorKind
// ---------------------------------------------------------------------------

/// Category of E2E check failure.
//fusa:req REQ-SAFETY-004, REQ-SAFETY-005, REQ-SAFETY-006
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum E2EErrorKind {
    /// The computed CRC does not match the header CRC.
    CrcMismatch,
    /// One or more sequence counter values were skipped.
    SequenceGap,
    /// The received data is shorter than the 10-byte header.
    HeaderTooShort,
}

impl std::fmt::Display for E2EErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            E2EErrorKind::CrcMismatch => write!(f, "CRC mismatch"),
            E2EErrorKind::SequenceGap => write!(f, "sequence gap"),
            E2EErrorKind::HeaderTooShort => write!(f, "header too short"),
        }
    }
}

// ---------------------------------------------------------------------------
// E2EError
// ---------------------------------------------------------------------------

/// An E2E safety check failure.
//fusa:req REQ-SAFETY-004, REQ-SAFETY-005, REQ-SAFETY-006
#[derive(Debug)]
pub struct E2EError {
    pub kind: E2EErrorKind,
    pub counter: u32,
    pub message: String,
}

impl std::fmt::Display for E2EError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "can/safety: E2E {} (counter={}): {}",
            self.kind, self.counter, self.message
        )
    }
}

impl std::error::Error for E2EError {}

// ---------------------------------------------------------------------------
// Protector
// ---------------------------------------------------------------------------

/// Prepends an E2E header to payloads before transmission.
//fusa:req REQ-SAFETY-001, REQ-SAFETY-002, REQ-SAFETY-003
pub struct Protector {
    cfg: Config,
    seq: AtomicU32,
}

impl Protector {
    /// Create a new protector.
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            seq: AtomicU32::new(0),
        }
    }

    /// Prepend the E2E header and return the protected payload.
    ///
    /// The sequence counter increments monotonically on each call.
    //fusa:req REQ-SAFETY-001, REQ-SAFETY-002, REQ-SAFETY-003
    pub fn protect(&self, payload: &[u8]) -> Vec<u8> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let header = build_header(self.cfg.data_id, self.cfg.source_id, seq, payload);

        let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(payload);
        out
    }
}

// ---------------------------------------------------------------------------
// Receiver
// ---------------------------------------------------------------------------

/// Validates E2E headers and strips them from received data.
//fusa:req REQ-SAFETY-004, REQ-SAFETY-005, REQ-SAFETY-006
pub struct Receiver {
    cfg: Config,
    state: Mutex<ReceiverState>,
}

struct ReceiverState {
    last_seq: u32,
    first: bool,
}

impl Receiver {
    /// Create a new receiver.
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            state: Mutex::new(ReceiverState {
                last_seq: 0,
                first: true,
            }),
        }
    }

    /// Validate the E2E header and return the original payload.
    ///
    /// # Errors
    ///
    /// - `E2EErrorKind::HeaderTooShort` — data shorter than 10 bytes.
    /// - `E2EErrorKind::CrcMismatch` — CRC in the header does not match.
    /// - `E2EErrorKind::SequenceGap` — sequence counter is not consecutive.
    //fusa:req REQ-SAFETY-004, REQ-SAFETY-005, REQ-SAFETY-006
    pub fn unwrap(&self, data: &[u8]) -> Result<Vec<u8>, E2EError> {
        //fusa:req REQ-SAFETY-006
        if data.len() < HEADER_SIZE {
            return Err(E2EError {
                kind: E2EErrorKind::HeaderTooShort,
                counter: 0,
                message: format!("need {} bytes, got {}", HEADER_SIZE, data.len()),
            });
        }

        let seq = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let received_crc = u16::from_le_bytes([data[8], data[9]]);
        let payload = &data[HEADER_SIZE..];

        //fusa:req REQ-SAFETY-004: verify CRC.
        let expected_header = build_header(self.cfg.data_id, self.cfg.source_id, seq, payload);
        let expected_crc = u16::from_le_bytes([expected_header[8], expected_header[9]]);
        if received_crc != expected_crc {
            return Err(E2EError {
                kind: E2EErrorKind::CrcMismatch,
                counter: seq,
                message: format!(
                    "CRC mismatch: received=0x{:04X} expected=0x{:04X}",
                    received_crc, expected_crc
                ),
            });
        }

        //fusa:req REQ-SAFETY-005: check sequence counter.
        let mut state = self.state.lock().unwrap();
        if !state.first && seq != state.last_seq.wrapping_add(1) {
            let expected = state.last_seq.wrapping_add(1);
            state.last_seq = seq;
            return Err(E2EError {
                kind: E2EErrorKind::SequenceGap,
                counter: seq,
                message: format!("expected counter {}, got {}", expected, seq),
            });
        }
        state.first = false;
        state.last_seq = seq;
        drop(state);

        Ok(payload.to_vec())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Build the 10-byte E2E header with CRC filled in.
///
/// The CRC slot (bytes 8–9) is treated as zero when computing the CRC,
/// then the computed CRC is written into the slot.
fn build_header(data_id: u16, source_id: u16, seq: u32, payload: &[u8]) -> [u8; HEADER_SIZE] {
    let mut hdr = [0u8; HEADER_SIZE];
    hdr[0..2].copy_from_slice(&data_id.to_le_bytes());
    hdr[2..4].copy_from_slice(&source_id.to_le_bytes());
    hdr[4..8].copy_from_slice(&seq.to_le_bytes());
    // hdr[8..10] = 0 during CRC computation.

    // CRC-16/CCITT-FALSE over the first 8 header bytes (CRC slot = 0) + payload.
    let mut crc = crc16_ccitt_false(&hdr[..8]);
    crc = crc16_ccitt_false_cont(crc, payload);

    hdr[8..10].copy_from_slice(&crc.to_le_bytes());
    hdr
}

/// Continue a CRC-16/CCITT-FALSE computation over additional data.
fn crc16_ccitt_false_cont(mut crc: u16, data: &[u8]) -> u16 {
    const POLY: u16 = 0x1021;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ POLY;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pair() -> (Protector, Receiver) {
        let cfg = Config {
            data_id: 0x1234,
            source_id: 0x5678,
        };
        (Protector::new(cfg), Receiver::new(cfg))
    }

    //fusa:req REQ-SAFETY-001
    #[test]
    fn header_prepended() {
        let (p, _r) = make_pair();
        let payload = b"hello";
        let protected = p.protect(payload);
        assert_eq!(protected.len(), HEADER_SIZE + payload.len());
        // Payload is at the end.
        assert_eq!(&protected[HEADER_SIZE..], payload);
    }

    //fusa:req REQ-SAFETY-002, REQ-SAFETY-003, REQ-SAFETY-004
    #[test]
    fn protect_and_unwrap() {
        let (p, r) = make_pair();
        let payload = b"test data";
        let protected = p.protect(payload);
        let recovered = r.unwrap(&protected).unwrap();
        assert_eq!(recovered, payload);
    }

    //fusa:req REQ-SAFETY-003
    #[test]
    fn sequence_counter_increments() {
        let (p, r) = make_pair();
        let p1 = p.protect(b"a");
        let p2 = p.protect(b"b");
        let p3 = p.protect(b"c");

        r.unwrap(&p1).unwrap();
        r.unwrap(&p2).unwrap();
        r.unwrap(&p3).unwrap();
    }

    //fusa:req REQ-SAFETY-004
    #[test]
    fn crc_mismatch_detected() {
        let (p, r) = make_pair();
        let mut protected = p.protect(b"hello");
        // Corrupt a payload byte.
        protected[HEADER_SIZE] ^= 0xFF;
        let err = r.unwrap(&protected).unwrap_err();
        assert_eq!(err.kind, E2EErrorKind::CrcMismatch);
    }

    //fusa:req REQ-SAFETY-005
    #[test]
    fn sequence_gap_detected() {
        let (p, r) = make_pair();
        let p0 = p.protect(b"frame 0");
        let _p1 = p.protect(b"frame 1");
        let p2 = p.protect(b"frame 2");

        // Receive frame 0 OK.
        r.unwrap(&p0).unwrap();
        // Skip frame 1, send frame 2 — should detect gap.
        let err = r.unwrap(&p2).unwrap_err();
        assert_eq!(err.kind, E2EErrorKind::SequenceGap);
    }

    //fusa:req REQ-SAFETY-006
    #[test]
    fn header_too_short_detected() {
        let (_p, r) = make_pair();
        let err = r.unwrap(&[0u8; 5]).unwrap_err();
        assert_eq!(err.kind, E2EErrorKind::HeaderTooShort);
    }

    #[test]
    fn empty_payload_works() {
        let (p, r) = make_pair();
        let protected = p.protect(&[]);
        let recovered = r.unwrap(&protected).unwrap();
        assert!(recovered.is_empty());
    }
}
