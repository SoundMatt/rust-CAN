// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! ISO 15765-2 (ISO-TP) transport protocol over CAN.
//!
//! Supports payloads up to 4095 bytes using single-frame, first-frame,
//! consecutive-frame, and flow-control PDU types.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::bus::Bus;
use crate::error::Error;
use crate::frame::{Filter, Frame};
use crate::relay::{Context, SubscriberOptions};

// ---------------------------------------------------------------------------
// Frame type nibbles
// ---------------------------------------------------------------------------

const TYPE_SF: u8 = 0x00; // Single Frame
const TYPE_FF: u8 = 0x10; // First Frame
const TYPE_CF: u8 = 0x20; // Consecutive Frame
const TYPE_FC: u8 = 0x30; // Flow Control

// Flow control status nibbles
const FC_CTS: u8 = 0x00; // Continue to Send
const FC_WAIT: u8 = 0x01; // Wait
const FC_OVERFLOW: u8 = 0x02; // Overflow

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// ISO-TP connection configuration.
//fusa:req REQ-ISOTP-001
//fusa:req REQ-ISOTP-002
//fusa:req REQ-ISOTP-003
#[derive(Debug, Clone)]
pub struct Config {
    /// CAN ID for outgoing frames.
    pub tx_id: u32,
    /// CAN ID expected for incoming frames.
    pub rx_id: u32,
    /// Use 29-bit extended CAN IDs.
    pub ext_ids: bool,
    /// Maximum consecutive frames per block (0 = unlimited).
    pub block_size: u8,
    /// Minimum separation time between consecutive frames.
    /// 0–127 = milliseconds; 0xF1–0xF9 = 100–900 µs.
    pub st_min: u8,
    /// Maximum wait time for a flow-control or consecutive frame.
    /// Defaults to 250 ms.
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tx_id: 0x7E0,
            rx_id: 0x7E8,
            ext_ids: false,
            block_size: 0,
            st_min: 0,
            timeout: Duration::from_millis(250),
        }
    }
}

impl Config {
    fn effective_timeout(&self) -> Duration {
        if self.timeout == Duration::ZERO {
            Duration::from_millis(250)
        } else {
            self.timeout
        }
    }
}

// ---------------------------------------------------------------------------
// IsoTpConn
// ---------------------------------------------------------------------------

/// An ISO-TP connection for multi-frame message transfer over CAN.
///
/// The connection subscribes to the rx channel at construction time, so
/// frames are buffered from the moment the connection is created.
//fusa:req REQ-ISOTP-004
pub struct IsoTpConn {
    bus: Arc<dyn Bus>,
    cfg: Config,
    /// Pre-created subscription for inbound ISO-TP frames.
    rx: crate::bus::FrameReceiver,
}

impl IsoTpConn {
    /// Create a new ISO-TP connection over the given bus.
    ///
    /// Subscribes to `cfg.rx_id` at construction time so that inbound frames
    /// are buffered before the first call to `recv()`.
    //fusa:req REQ-ISOTP-001
    //fusa:req REQ-ISOTP-002
    pub async fn new(bus: Arc<dyn Bus>, cfg: Config) -> Result<Self, Error> {
        // Validate config.
        if cfg.tx_id == cfg.rx_id {
            return Err(Error::Other("isotp: tx_id and rx_id must differ".into()));
        }

        let rx = bus
            .subscribe(
                vec![Filter {
                    id: cfg.rx_id,
                    mask: if cfg.ext_ids { 0x1FFF_FFFF } else { 0x7FF },
                }],
                SubscriberOptions::default(),
            )
            .await?;

        Ok(Self { bus, cfg, rx })
    }

    /// Send a payload using ISO-TP segmentation.
    ///
    /// Payloads ≤ 7 bytes are sent as a single frame.
    /// Larger payloads are segmented and flow-controlled.
    //fusa:req REQ-ISOTP-001
    //fusa:req REQ-ISOTP-002
    //fusa:req REQ-ISOTP-003
    pub async fn send(&self, _ctx: Context, payload: &[u8]) -> Result<(), Error> {
        if payload.is_empty() {
            return Err(Error::Other("isotp: empty payload".into()));
        }
        if payload.len() > 4095 {
            return Err(Error::PayloadTooLarge);
        }

        if payload.len() <= 7 {
            self.send_single_frame(payload).await
        } else {
            self.send_multi_frame(payload).await
        }
    }

    /// Receive the next ISO-TP message, reassembling multi-frame sequences.
    //fusa:req REQ-ISOTP-004
    //fusa:req REQ-ISOTP-005
    pub async fn recv(&self, _ctx: Context) -> Result<Vec<u8>, Error> {
        let tmo = self.cfg.effective_timeout();

        // Wait for the first frame.
        let first = timeout(tmo, self.rx.recv())
            .await
            .map_err(|_| Error::Timeout)?
            .ok_or(Error::Closed)?;

        if first.data.is_empty() {
            return Err(Error::Other("isotp: empty first frame".into()));
        }

        let pci = first.data[0] & 0xF0;

        match pci {
            // Single Frame
            0x00 => {
                let len = (first.data[0] & 0x0F) as usize;
                if len == 0 || len > 7 || len > first.data.len() - 1 {
                    return Err(Error::Other("isotp: invalid SF length".into()));
                }
                Ok(first.data[1..1 + len].to_vec())
            }
            // First Frame
            0x10 => {
                if first.data.len() < 2 {
                    return Err(Error::Other("isotp: FF too short".into()));
                }
                let len = (((first.data[0] & 0x0F) as usize) << 8) | (first.data[1] as usize);
                let mut buf = Vec::with_capacity(len);
                buf.extend_from_slice(&first.data[2..]);

                // Send flow control: ContinueToSend.
                let fc_frame =
                    self.make_frame(vec![TYPE_FC | FC_CTS, self.cfg.block_size, self.cfg.st_min]);
                self.bus.send(Context::background(), fc_frame).await?;

                // Receive consecutive frames.
                let mut sn: u8 = 1;
                let mut cf_in_block: u8 = 0;
                while buf.len() < len {
                    let cf = timeout(tmo, self.rx.recv())
                        .await
                        .map_err(|_| Error::Timeout)?
                        .ok_or(Error::Closed)?;

                    if cf.data.is_empty() {
                        return Err(Error::Other("isotp: empty CF".into()));
                    }
                    if cf.data[0] & 0xF0 != TYPE_CF {
                        return Err(Error::Other("isotp: expected CF".into()));
                    }
                    let cf_sn = cf.data[0] & 0x0F;
                    if cf_sn != sn & 0x0F {
                        return Err(Error::Other(format!(
                            "isotp: SN mismatch (expected {}, got {})",
                            sn & 0x0F,
                            cf_sn
                        )));
                    }
                    sn = sn.wrapping_add(1);

                    let remaining = len - buf.len();
                    let chunk = &cf.data[1..];
                    let take = chunk.len().min(remaining);
                    buf.extend_from_slice(&chunk[..take]);

                    // Per ISO 15765-2, a non-zero BlockSize obligates the
                    // receiver to issue a fresh CTS flow-control frame after
                    // every `block_size` consecutive frames. Without this a
                    // conformant sender configured with BS>0 stalls at the
                    // block boundary waiting for the next FC.
                    cf_in_block = cf_in_block.wrapping_add(1);
                    if self.cfg.block_size != 0
                        && cf_in_block == self.cfg.block_size
                        && buf.len() < len
                    {
                        let fc_frame = self.make_frame(vec![
                            TYPE_FC | FC_CTS,
                            self.cfg.block_size,
                            self.cfg.st_min,
                        ]);
                        self.bus.send(Context::background(), fc_frame).await?;
                        cf_in_block = 0;
                    }
                }

                Ok(buf)
            }
            _ => Err(Error::Other(format!(
                "isotp: unexpected frame type 0x{:02X}",
                pci
            ))),
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    async fn send_single_frame(&self, payload: &[u8]) -> Result<(), Error> {
        let mut data = Vec::with_capacity(payload.len() + 1);
        data.push(TYPE_SF | (payload.len() as u8));
        data.extend_from_slice(payload);
        self.bus
            .send(Context::background(), self.make_frame(data))
            .await
    }

    async fn send_multi_frame(&self, payload: &[u8]) -> Result<(), Error> {
        let tmo = self.cfg.effective_timeout();

        // First Frame.
        let ff_len = payload.len();
        let mut ff = Vec::with_capacity(8);
        ff.push(TYPE_FF | (((ff_len >> 8) & 0x0F) as u8));
        ff.push((ff_len & 0xFF) as u8);
        let first_chunk_len = (payload.len()).min(6);
        ff.extend_from_slice(&payload[..first_chunk_len]);
        self.bus
            .send(Context::background(), self.make_frame(ff))
            .await?;

        let mut payload = &payload[first_chunk_len..];

        //fusa:req REQ-ISOTP-003
        let mut fc = self.wait_fc(tmo).await?;
        if fc[0] & 0x0F == FC_OVERFLOW {
            return Err(Error::Other("isotp: receiver overflow".into()));
        }

        let mut sn: u8 = 1;
        let mut block_count: u32 = 0;

        while !payload.is_empty() {
            // Handle wait frames: block until the receiver sends a non-WAIT
            // FC, adopting whatever BlockSize/STmin it carries — a WAIT can
            // legitimately be followed by another WAIT, so this must loop
            // rather than assume a single re-check clears it.
            while fc[0] & 0x0F == FC_WAIT {
                fc = self.wait_fc(tmo).await?;
                if fc[0] & 0x0F == FC_OVERFLOW {
                    return Err(Error::Other("isotp: receiver overflow".into()));
                }
                block_count = 0;
            }

            let chunk_len = payload.len().min(7);
            let chunk = &payload[..chunk_len];

            let mut cf = Vec::with_capacity(8);
            cf.push(TYPE_CF | (sn & 0x0F));
            cf.extend_from_slice(chunk);
            self.bus
                .send(Context::background(), self.make_frame(cf))
                .await?;

            payload = &payload[chunk_len..];
            sn = sn.wrapping_add(1);
            block_count += 1;

            // ST_min delay, per the most recently received FC.
            let st = st_min_to_duration(fc[2]);
            if !st.is_zero() {
                tokio::time::sleep(st).await;
            }

            // Block size check: if block_size > 0 and we've sent that many,
            // wait for the next flow control frame and adopt its
            // BlockSize/STmin for the remainder of the transfer — ISO-TP
            // allows (and real ECUs commonly do) the receiver to change
            // pacing parameters mid-transfer, so a later FC must not be
            // discarded in favor of the first one.
            let bs = fc[1];
            if bs > 0 && block_count >= bs as u32 && !payload.is_empty() {
                fc = self.wait_fc(tmo).await?;
                if fc[0] & 0x0F == FC_OVERFLOW {
                    return Err(Error::Other("isotp: receiver overflow".into()));
                }
                block_count = 0;
            }
        }

        Ok(())
    }

    async fn wait_fc(&self, tmo: Duration) -> Result<Vec<u8>, Error> {
        //fusa:req REQ-ISOTP-005
        //fusa:req REQ-SEC-004
        let f = timeout(tmo, self.rx.recv())
            .await
            .map_err(|_| Error::Timeout)?
            .ok_or(Error::Closed)?;

        // A Flow Control PDU is at minimum 3 bytes (PCI/FS, BlockSize,
        // STmin) — a malformed or truncated FC must be rejected here rather
        // than let callers index into it and panic (a peer sending a
        // short/garbage FC must not be able to crash the sender).
        if f.data.len() < 3 || f.data[0] & 0xF0 != TYPE_FC {
            return Err(Error::Other(
                "isotp: malformed or truncated flow control frame".into(),
            ));
        }
        Ok(f.data.clone())
    }

    fn make_frame(&self, data: Vec<u8>) -> Frame {
        Frame {
            id: self.cfg.tx_id,
            ext: self.cfg.ext_ids,
            data,
            ..Default::default()
        }
    }
}

/// Decode STmin field to a `Duration`.
///
/// 0x00–0x7F → 0–127 ms
/// 0xF1–0xF9 → 100–900 µs
/// All other values → 0 (no delay)
fn st_min_to_duration(st_min: u8) -> Duration {
    match st_min {
        0x00..=0x7F => Duration::from_millis(st_min as u64),
        0xF1..=0xF9 => Duration::from_micros((st_min - 0xF0) as u64 * 100),
        _ => Duration::ZERO,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::virtual_bus::VirtualBus;

    fn make_bus() -> Arc<VirtualBus> {
        Arc::new(VirtualBus::new())
    }

    fn make_cfg(tx_id: u32, rx_id: u32) -> Config {
        Config {
            tx_id,
            rx_id,
            timeout: Duration::from_millis(100),
            ..Default::default()
        }
    }

    //fusa:test REQ-ISOTP-001
    #[tokio::test]
    async fn single_frame_send_receive() {
        let bus = make_bus();
        let sender_cfg = make_cfg(0x7E0, 0x7E8);
        let receiver_cfg = make_cfg(0x7E8, 0x7E0);

        let sender = IsoTpConn::new(bus.clone(), sender_cfg).await.unwrap();
        let receiver = IsoTpConn::new(bus.clone(), receiver_cfg).await.unwrap();

        let payload = b"Hello!!"; // 7 bytes → single frame

        let recv_task = tokio::spawn(async move { receiver.recv(Context::background()).await });

        sender.send(Context::background(), payload).await.unwrap();

        let received = recv_task.await.unwrap().unwrap();
        assert_eq!(received, payload);
    }

    //fusa:test REQ-ISOTP-002
    //fusa:test REQ-ISOTP-003
    //fusa:test REQ-ISOTP-004
    #[tokio::test]
    async fn multi_frame_send_receive() {
        let bus = make_bus();
        let sender_cfg = make_cfg(0x7E0, 0x7E8);
        let receiver_cfg = make_cfg(0x7E8, 0x7E0);

        let sender = IsoTpConn::new(bus.clone(), sender_cfg).await.unwrap();
        let receiver = IsoTpConn::new(bus.clone(), receiver_cfg).await.unwrap();

        // 20 bytes — requires first frame + consecutive frames.
        let payload: Vec<u8> = (0..20).collect();
        let payload_clone = payload.clone();

        let recv_task = tokio::spawn(async move { receiver.recv(Context::background()).await });

        sender.send(Context::background(), &payload).await.unwrap();

        let received = recv_task.await.unwrap().unwrap();
        assert_eq!(received, payload_clone);
    }

    #[test]
    fn st_min_decode() {
        assert_eq!(st_min_to_duration(0), Duration::from_millis(0));
        assert_eq!(st_min_to_duration(10), Duration::from_millis(10));
        assert_eq!(st_min_to_duration(127), Duration::from_millis(127));
        assert_eq!(st_min_to_duration(0xF1), Duration::from_micros(100));
        assert_eq!(st_min_to_duration(0xF9), Duration::from_micros(900));
        assert_eq!(st_min_to_duration(0x80), Duration::ZERO);
    }

    //fusa:test REQ-ISOTP-005
    #[tokio::test]
    async fn recv_timeout() {
        let bus = make_bus();
        let cfg = Config {
            tx_id: 0x7E0,
            rx_id: 0x7E8,
            timeout: Duration::from_millis(10),
            ..Default::default()
        };
        let conn = IsoTpConn::new(bus, cfg).await.unwrap();
        let result = conn.recv(Context::background()).await;
        assert!(matches!(result, Err(Error::Timeout)));
    }

    /// A malformed/truncated Flow Control frame (fewer than 3 bytes) must be
    /// rejected as a protocol error, not cause an index-out-of-bounds panic
    /// in `send_multi_frame()`. Regression test for a peer replying to a
    /// First Frame with a 1-byte FC frame.
    //fusa:test REQ-ISOTP-003
    //fusa:test REQ-ISOTP-005
    #[tokio::test]
    async fn malformed_flow_control_frame_is_rejected_not_panicking() {
        let bus = make_bus();
        let sender = IsoTpConn::new(bus.clone(), make_cfg(0x7E0, 0x7E8))
            .await
            .unwrap();

        // Fake peer: listens for the sender's frames on 0x7E0 and answers
        // its First Frame with a truncated (1-byte) FC frame.
        let peer_rx = bus
            .subscribe(
                vec![Filter {
                    id: 0x7E0,
                    mask: 0x7FF,
                }],
                SubscriberOptions::default(),
            )
            .await
            .unwrap();

        // 20 bytes → requires a multi-frame send.
        let payload: Vec<u8> = (0..20).collect();
        let send_task =
            tokio::spawn(async move { sender.send(Context::background(), &payload).await });

        let ff = peer_rx.recv().await.unwrap();
        assert_eq!(ff.data[0] & 0xF0, TYPE_FF, "expected a First Frame");

        let bad_fc = Frame {
            id: 0x7E8,
            data: vec![TYPE_FC | FC_CTS], // 1 byte — missing BlockSize/STmin
            ..Default::default()
        };
        bus.send(Context::background(), bad_fc).await.unwrap();

        // Must not panic: send_task.await() itself would return a JoinError
        // (panic) if the fix regressed.
        let result = send_task.await.expect("sender task must not panic");
        assert!(
            matches!(result, Err(Error::Other(_))),
            "expected a protocol error, got {result:?}"
        );
    }

    /// The sender must adopt updated BlockSize/STmin from a Flow Control
    /// frame received mid-transfer, not keep reusing the first FC forever.
    /// Regression test: the peer initially throttles to BlockSize=1, then
    /// raises it to BlockSize=2 on the next FC; a sender that ignores the
    /// update will request a third FC that the peer never sends, and the
    /// send will time out instead of completing.
    //fusa:test REQ-ISOTP-003
    #[tokio::test]
    async fn mid_transfer_flow_control_update_is_honored() {
        let bus = make_bus();
        let sender_cfg = make_cfg(0x7E0, 0x7E8); // 100 ms timeout
        let sender = IsoTpConn::new(bus.clone(), sender_cfg).await.unwrap();

        let peer_rx = bus
            .subscribe(
                vec![Filter {
                    id: 0x7E0,
                    mask: 0x7FF,
                }],
                SubscriberOptions::default(),
            )
            .await
            .unwrap();

        // 6 bytes in the FF + 3 consecutive frames of 7 bytes each = 27 bytes.
        let payload: Vec<u8> = (0..27).collect();
        let send_task =
            tokio::spawn(async move { sender.send(Context::background(), &payload).await });

        // First Frame, then answer with BlockSize=1 (send one CF, then wait).
        let ff = peer_rx.recv().await.unwrap();
        assert_eq!(ff.data[0] & 0xF0, TYPE_FF);
        bus.send(
            Context::background(),
            Frame {
                id: 0x7E8,
                data: vec![TYPE_FC | FC_CTS, 1, 0],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // First CF, then raise BlockSize to 2 for the remaining two CFs.
        let cf1 = peer_rx.recv().await.unwrap();
        assert_eq!(cf1.data[0] & 0xF0, TYPE_CF);
        bus.send(
            Context::background(),
            Frame {
                id: 0x7E8,
                data: vec![TYPE_FC | FC_CTS, 2, 0],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // If the sender correctly adopted BlockSize=2, it sends the
        // remaining two CFs back-to-back and completes without requesting
        // a third FC (which this test never sends).
        let result = tokio::time::timeout(Duration::from_millis(500), send_task)
            .await
            .expect("send task should complete without needing another FC")
            .expect("sender task must not panic");
        assert!(result.is_ok(), "expected send to succeed, got {result:?}");
    }
}
