// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! SAE J1939 protocol layer over CAN.
//!
//! J1939 uses 29-bit extended CAN IDs with a structured layout:
//!
//! ```text
//! Bits 28–26  Priority (3 bits)
//! Bit  25     Reserved
//! Bit  24     Data Page (DP)
//! Bits 23–16  PDU Format (PF, 8 bits)
//! Bits 15–8   PDU Specific (PS, 8 bits) — destination if PF<240, group ext if PF≥240
//! Bits  7–0   Source Address (SA, 8 bits)
//! ```

use std::sync::Arc;

use crate::bus::{Bus, FrameReceiver};
use crate::error::Error;
use crate::frame::Frame;
use crate::relay::{Context, SubscriberOptions};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// J1939 global broadcast destination address.
//fusa:req REQ-J1939-002
pub const BROADCAST_ADDR: u8 = 0xFF;

/// J1939 null address (not claimed).
pub const NULL_ADDR: u8 = 0xFE;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// J1939 message priority (0 = highest, 7 = lowest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Priority(pub u8);

impl Priority {
    /// Default application priority per J1939.
    pub const DEFAULT: Priority = Priority(6);

    /// Returns the raw priority value (0–7).
    pub fn value(self) -> u8 {
        self.0 & 0x07
    }
}

impl Default for Priority {
    fn default() -> Self {
        Priority::DEFAULT
    }
}

/// A J1939 Parameter Group Number (18 bits effective).
//fusa:req REQ-J1939-001
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pgn(pub u32);

impl Pgn {
    /// Returns true when PDU Format < 240 (peer-to-peer message).
    //fusa:req REQ-J1939-002
    pub fn is_peer_to_peer(self) -> bool {
        let pf = ((self.0 >> 8) & 0xFF) as u8;
        pf < 240
    }
}

impl std::fmt::Display for Pgn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PGN({:#06X})", self.0)
    }
}

/// A decoded J1939 frame.
//fusa:req REQ-J1939-001
#[derive(Debug, Clone)]
pub struct J1939Frame {
    pub priority: Priority,
    pub pgn: Pgn,
    /// Source address (SA).
    pub src: u8,
    /// Destination address (PS, valid only when pgn.is_peer_to_peer()).
    pub dst: u8,
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// decode_id / encode_id
// ---------------------------------------------------------------------------

/// Decode priority, PGN, and source address from a 29-bit extended CAN ID.
///
/// For peer-to-peer (PF < 240): PS is the destination address; PGN excludes PS.
/// For broadcast (PF ≥ 240): PS is the group extension, included in PGN.
//fusa:req REQ-J1939-001
//fusa:req REQ-J1939-002
//fusa:req REQ-J1939-003
pub fn decode_id(id: u32) -> (Priority, Pgn, u8) {
    let priority = Priority((id >> 26) as u8 & 0x07);
    let src = (id & 0xFF) as u8;
    let pf = ((id >> 16) & 0xFF) as u8;
    let ps = ((id >> 8) & 0xFF) as u8;
    let dp = ((id >> 24) & 0x01) as u8;

    let pgn = if pf < 240 {
        // Peer-to-peer: PS is destination, not part of PGN.
        Pgn((dp as u32) << 17 | (pf as u32) << 8)
    } else {
        // Broadcast: PS is group extension, is part of PGN.
        Pgn((dp as u32) << 17 | (pf as u32) << 8 | ps as u32)
    };

    (priority, pgn, src)
}

/// Encode priority, PGN, source, and destination into a 29-bit extended CAN ID.
//fusa:req REQ-J1939-004
pub fn encode_id(priority: Priority, pgn: Pgn, src: u8, dst: u8) -> u32 {
    let pf = ((pgn.0 >> 8) & 0xFF) as u8;
    let ps = (pgn.0 & 0xFF) as u8;
    let dp = ((pgn.0 >> 17) & 0x01) as u8;

    let mut id: u32 = 0;
    id |= (priority.value() as u32) << 26;
    id |= (dp as u32) << 24;
    id |= (pf as u32) << 16;
    if pf >= 240 {
        // Broadcast: include group extension from PGN.
        id |= (ps as u32) << 8;
    } else {
        // Peer-to-peer: destination goes in PS field.
        id |= (dst as u32) << 8;
    }
    id |= src as u32;
    id
}

// ---------------------------------------------------------------------------
// J1939Bus
// ---------------------------------------------------------------------------

/// A J1939-aware CAN bus wrapping a lower-level `Bus` implementation.
//fusa:req REQ-J1939-005
pub struct J1939Bus {
    bus: Arc<dyn Bus>,
    /// This node's source address.
    pub source_address: u8,
}

impl J1939Bus {
    /// Create a J1939 bus with the given source address.
    pub fn new(bus: Arc<dyn Bus>, source_address: u8) -> Self {
        Self {
            bus,
            source_address,
        }
    }

    /// Send a J1939 frame.
    //fusa:req REQ-J1939-005
    pub async fn send(&self, ctx: Context, frame: J1939Frame) -> Result<(), Error> {
        let id = encode_id(frame.priority, frame.pgn, self.source_address, frame.dst);
        self.bus
            .send(
                ctx,
                Frame {
                    id,
                    ext: true,
                    data: frame.data,
                    ..Default::default()
                },
            )
            .await
    }

    /// Subscribe to J1939 frames, optionally filtered by PGN.
    ///
    /// Pass an empty slice for `pgns` to receive all J1939 frames.
    //fusa:req REQ-J1939-005
    //fusa:req REQ-J1939-006
    pub async fn subscribe(
        &self,
        pgns: Vec<Pgn>,
        opts: SubscriberOptions,
    ) -> Result<J1939Receiver, Error> {
        let rx = self.bus.subscribe(vec![], opts).await?;
        Ok(J1939Receiver { rx, pgns })
    }

    /// Close the underlying bus.
    pub async fn close(&self) -> Result<(), Error> {
        self.bus.close().await
    }
}

// ---------------------------------------------------------------------------
// J1939Receiver
// ---------------------------------------------------------------------------

/// A J1939 subscription that decodes raw CAN frames into J1939 frames.
//fusa:req REQ-J1939-006
pub struct J1939Receiver {
    rx: FrameReceiver,
    pgns: Vec<Pgn>,
}

impl J1939Receiver {
    /// Receive the next J1939 frame.
    ///
    /// Non-J1939 (standard, non-extended) frames are silently skipped.
    /// Returns `None` when the bus is closed.
    pub async fn recv(&self) -> Option<J1939Frame> {
        loop {
            let f = self.rx.recv().await?;

            // J1939 uses extended CAN IDs only.
            if !f.ext {
                continue;
            }

            let (priority, pgn, src) = decode_id(f.id);

            //fusa:req REQ-J1939-006
            if !self.pgns.is_empty() && !self.pgns.contains(&pgn) {
                continue;
            }

            let dst = if pgn.is_peer_to_peer() {
                ((f.id >> 8) & 0xFF) as u8
            } else {
                BROADCAST_ADDR
            };

            return Some(J1939Frame {
                priority,
                pgn,
                src,
                dst,
                data: f.data,
            });
        }
    }

    /// Close this receiver.
    pub fn close(&self) {
        self.rx.close();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    //fusa:test REQ-J1939-002
    //fusa:test REQ-J1939-003
    #[test]
    fn decode_peer_to_peer() {
        // PF = 0xEC (236 < 240) → peer-to-peer
        // Priority=6, DP=0, PF=0xEC, PS=0x00 (dst), SA=0x01
        let id: u32 = (6 << 26) | (0xEC << 16) | 0x01; // PS=0x00 is elided (no-op)
        let (priority, pgn, src) = decode_id(id);
        assert_eq!(priority.value(), 6);
        assert!(pgn.is_peer_to_peer());
        assert_eq!(src, 0x01);
    }

    //fusa:test REQ-J1939-002
    //fusa:test REQ-J1939-003
    #[test]
    fn decode_broadcast() {
        // PF = 0xFE (254 ≥ 240) → broadcast
        // Priority=6, DP=0, PF=0xFE, PS=0x00, SA=0x01
        let id: u32 = (6 << 26) | (0xFE << 16) | 0x01; // PS=0x00 is elided (no-op)
        let (priority, pgn, src) = decode_id(id);
        assert_eq!(priority.value(), 6);
        assert!(!pgn.is_peer_to_peer());
        assert_eq!(src, 0x01);
    }

    //fusa:test REQ-J1939-001
    //fusa:test REQ-J1939-004
    #[test]
    fn encode_decode_roundtrip_broadcast() {
        let priority = Priority(3);
        let pgn = Pgn(0x0FE00); // PF = 0xFE ≥ 240
        let src = 0x22;
        let dst = BROADCAST_ADDR;

        let id = encode_id(priority, pgn, src, dst);
        let (p2, pgn2, src2) = decode_id(id);

        assert_eq!(p2.value(), 3);
        assert_eq!(pgn2, pgn);
        assert_eq!(src2, src);
    }

    //fusa:test REQ-J1939-001
    //fusa:test REQ-J1939-004
    #[test]
    fn encode_decode_roundtrip_peer() {
        let priority = Priority(6);
        let pgn = Pgn(0x0EC00); // PF = 0xEC < 240
        let src = 0x10;
        let dst = 0x20;

        let id = encode_id(priority, pgn, src, dst);
        let (p2, pgn2, src2) = decode_id(id);

        assert_eq!(p2.value(), 6);
        assert_eq!(pgn2, pgn);
        assert_eq!(src2, src);
        // dst is in PS field
        assert_eq!(((id >> 8) & 0xFF) as u8, dst);
    }

    //fusa:test REQ-J1939-005
    //fusa:test REQ-J1939-006
    #[tokio::test]
    async fn send_and_receive_j1939() {
        use crate::virtual_bus::VirtualBus;
        let bus = Arc::new(VirtualBus::new());
        let j_bus = J1939Bus::new(bus.clone(), 0x01);

        let rx = j_bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();

        let frame = J1939Frame {
            priority: Priority(6),
            pgn: Pgn(0x0FEF1), // broadcast PGN (PF=0xFE ≥ 240)
            src: 0x01,
            dst: BROADCAST_ADDR,
            data: vec![1, 2, 3],
        };

        j_bus.send(Context::background(), frame).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.data, vec![1, 2, 3]);
    }

    //fusa:test REQ-J1939-006
    #[tokio::test]
    async fn pgn_filter_works() {
        use crate::virtual_bus::VirtualBus;
        let bus = Arc::new(VirtualBus::new());
        let j_bus = J1939Bus::new(bus.clone(), 0x01);

        let wanted_pgn = Pgn(0x0FEF1);
        let other_pgn = Pgn(0x0FEF2);

        let rx = j_bus
            .subscribe(vec![wanted_pgn], SubscriberOptions::default())
            .await
            .unwrap();

        // Send a frame with the "other" PGN — should be filtered out.
        let other_frame = J1939Frame {
            priority: Priority::default(),
            pgn: other_pgn,
            src: 0x01,
            dst: BROADCAST_ADDR,
            data: vec![0xFF],
        };
        j_bus
            .send(Context::background(), other_frame)
            .await
            .unwrap();

        // Send the wanted frame.
        let wanted_frame = J1939Frame {
            priority: Priority::default(),
            pgn: wanted_pgn,
            src: 0x01,
            dst: BROADCAST_ADDR,
            data: vec![0xAB],
        };
        j_bus
            .send(Context::background(), wanted_frame)
            .await
            .unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.pgn, wanted_pgn);
        assert_eq!(received.data, vec![0xAB]);
    }
}
