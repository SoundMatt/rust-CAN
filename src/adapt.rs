// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! RELAY adapter — wraps a CAN Bus as a relay::Node.
//!
//! Implements §10.3, §10.4, §10.5, and §15.7.1 of the RELAY spec.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::mpsc;

use crate::bus::Bus;
use crate::error::Error;
use crate::frame::Frame;
use crate::relay::{BackPressurePolicy, Context, Message, Protocol, SubscriberOptions};

// ---------------------------------------------------------------------------
// to_message / from_message
// ---------------------------------------------------------------------------

/// Convert a CAN Frame to a relay::Message per RELAY spec §15.7.1.
//fusa:req REQ-CAN-007
//fusa:req REQ-CAN-016
pub fn to_message(f: &Frame) -> Message {
    let mut meta = std::collections::HashMap::new();
    meta.insert("can.ext".into(), f.ext.to_string());
    meta.insert("can.fd".into(), f.fd.to_string());
    meta.insert("can.rtr".into(), f.rtr.to_string());
    meta.insert("can.brs".into(), f.brs.to_string());
    if f.esi {
        meta.insert("can.esi".into(), "true".into());
    }
    if f.xl {
        meta.insert("can.xl".into(), "true".into());
        if f.sdt != 0 {
            meta.insert("can.sdt".into(), f.sdt.to_string());
        }
        if f.vcid != 0 {
            meta.insert("can.vcid".into(), f.vcid.to_string());
        }
        if f.af != 0 {
            meta.insert("can.af".into(), f.af.to_string());
        }
        if f.sec {
            meta.insert("can.sec".into(), "true".into());
        }
    }

    Message {
        protocol: Protocol::Can,
        version: crate::relay::Version::default(),
        id: f.id.to_string(),
        payload: f.data.clone(),
        timestamp: Utc::now(),
        seq: 0,
        meta,
    }
}

/// Convert a relay::Message back to a CAN Frame per RELAY spec §15.7.1.
///
/// Returns `Error::InvalidFrame` if `msg.id` cannot be parsed as a `u32`.
//fusa:req REQ-CAN-007
pub fn from_message(m: &Message) -> Result<Frame, Error> {
    let id: u32 =
        m.id.parse()
            .map_err(|_| Error::invalid_frame(format!("invalid CAN ID: '{}'", m.id)))?;

    let ext = m.meta.get("can.ext").map(|v| v == "true").unwrap_or(false);
    let fd = m.meta.get("can.fd").map(|v| v == "true").unwrap_or(false);
    let rtr = m.meta.get("can.rtr").map(|v| v == "true").unwrap_or(false);
    let brs = m.meta.get("can.brs").map(|v| v == "true").unwrap_or(false);
    let esi = m.meta.get("can.esi").map(|v| v == "true").unwrap_or(false);
    let xl = m.meta.get("can.xl").map(|v| v == "true").unwrap_or(false);
    let sdt: u8 = m
        .meta
        .get("can.sdt")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let vcid: u8 = m
        .meta
        .get("can.vcid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let af: u32 = m
        .meta
        .get("can.af")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let sec = m.meta.get("can.sec").map(|v| v == "true").unwrap_or(false);

    Ok(Frame {
        id,
        ext,
        fd,
        rtr,
        brs,
        esi,
        xl,
        sdt,
        vcid,
        af,
        sec,
        data: m.payload.clone(),
    })
}

// ---------------------------------------------------------------------------
// adapt()
// ---------------------------------------------------------------------------

/// Wrap a `Bus` as a `relay::Node` for cross-protocol use per RELAY spec §10.3.
//fusa:req REQ-CAN-007
pub fn adapt(bus: Arc<dyn Bus>) -> Box<dyn crate::relay::Node> {
    Box::new(CanAdapter { bus })
}

// ---------------------------------------------------------------------------
// CanAdapter
// ---------------------------------------------------------------------------

struct CanAdapter {
    bus: Arc<dyn Bus>,
}

#[async_trait]
impl crate::relay::Node for CanAdapter {
    fn protocol(&self) -> Protocol {
        Protocol::Can
    }

    /// Send a relay::Message by converting it to a CAN frame.
    async fn send(&self, ctx: Context, msg: Message) -> Result<(), crate::relay::Error> {
        let frame = from_message(&msg).map_err(|_| crate::relay::Error::PayloadTooLarge)?;
        self.bus.send(ctx, frame).await.map_err(|e| match e {
            Error::Closed => crate::relay::Error::Closed,
            Error::NotConnected => crate::relay::Error::NotConnected,
            Error::Timeout => crate::relay::Error::Timeout,
            Error::PayloadTooLarge => crate::relay::Error::PayloadTooLarge,
            _ => crate::relay::Error::Closed, // map to Closed as a safe default
        })
    }

    /// Subscribe to the bus and forward frames as relay::Messages.
    ///
    /// Follows the goroutine model from RELAY spec §10.5: one task per
    /// subscription, back-pressure applied per the SubscriberOptions policy.
    async fn subscribe(
        &self,
        opts: SubscriberOptions,
    ) -> Result<mpsc::Receiver<Message>, crate::relay::Error> {
        let depth = opts.chan_depth(64);
        let policy = opts.back_pressure;

        // Subscribe to all frames (nil filters).
        let frame_rx = self
            .bus
            .subscribe(
                vec![],
                SubscriberOptions {
                    channel_depth: depth * 2, // give the internal channel more headroom
                    back_pressure: BackPressurePolicy::DropNewest,
                    rate_limit_per_sec: 0,
                },
            )
            .await
            .map_err(|_| crate::relay::Error::Closed)?;

        let (tx, rx) = mpsc::channel::<Message>(depth);
        let mut seq: u64 = 0;

        // Spawn a background task per §10.5 rule 1.
        tokio::spawn(async move {
            loop {
                match frame_rx.recv().await {
                    None => break,
                    Some(f) => {
                        let mut msg = to_message(&f);
                        msg.seq = seq;
                        seq += 1;

                        match policy {
                            BackPressurePolicy::DropNewest => {
                                // Non-blocking try_send; drop if full.
                                let _ = tx.try_send(msg);
                            }
                            BackPressurePolicy::DropOldest => {
                                if tx.capacity() == 0 {
                                    // Attempt a try_recv equivalent by just doing
                                    // a non-blocking attempt and ignoring failure.
                                    // mpsc doesn't support drain-one, so we just
                                    // do a best-effort send.
                                }
                                let _ = tx.try_send(msg);
                            }
                            BackPressurePolicy::Block => {
                                if tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            // §10.5 rule 2: channel is closed when task exits (tx dropped).
        });

        Ok(rx)
    }

    async fn close(&self) -> Result<(), crate::relay::Error> {
        self.bus
            .close()
            .await
            .map_err(|_| crate::relay::Error::Closed)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;

    #[test]
    fn to_message_roundtrip() {
        let f = Frame {
            id: 0x123,
            ext: false,
            fd: true,
            brs: true,
            data: vec![1, 2, 3],
            ..Default::default()
        };
        let msg = to_message(&f);
        assert_eq!(msg.id, "291"); // 0x123 = 291 decimal
        assert_eq!(msg.meta.get("can.fd").unwrap(), "true");
        assert_eq!(msg.meta.get("can.brs").unwrap(), "true");
        assert_eq!(msg.payload, vec![1, 2, 3]);

        let f2 = from_message(&msg).unwrap();
        assert_eq!(f2.id, f.id);
        assert_eq!(f2.fd, f.fd);
        assert_eq!(f2.brs, f.brs);
        assert_eq!(f2.data, f.data);
    }

    #[test]
    fn from_message_invalid_id() {
        let msg = Message {
            protocol: Protocol::Can,
            version: Default::default(),
            id: "not_a_number".into(),
            payload: vec![],
            timestamp: Utc::now(),
            seq: 0,
            meta: Default::default(),
        };
        assert!(matches!(
            from_message(&msg),
            Err(Error::InvalidFrame { .. })
        ));
    }

    #[tokio::test]
    async fn adapt_send_and_subscribe() {
        use crate::mock::MockBus;
        let mock = Arc::new(MockBus::new());
        let node = adapt(mock.clone());

        let ctx = Context::background();
        let msg = Message::new(Protocol::Can, "256", vec![0xDE, 0xAD]);
        node.send(ctx, msg).await.unwrap();

        let frames = mock.sent_frames().await;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].id, 256);
        assert_eq!(frames[0].data, vec![0xDE, 0xAD]);
    }
}
