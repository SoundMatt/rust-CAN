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
    let mut meta = std::collections::BTreeMap::new();
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
// AdaptQueue — policy-aware buffer for the Adapt()-level relay.Message
// channel, per RELAY spec §10.5 rule 3.
// ---------------------------------------------------------------------------

/// A bounded `Message` queue implementing `DropNewest`/`DropOldest`/`Block`
/// back-pressure, mirroring `bus::SubInner`'s (Frame-level) policy logic but
/// at the `relay.Message` layer that §10.5 rule 3 actually governs.
struct AdaptQueue {
    queue: std::sync::Mutex<std::collections::VecDeque<Message>>,
    capacity: usize,
    policy: BackPressurePolicy,
    /// Notified when an item is pushed (wakes a waiting `pop`).
    notify_push: tokio::sync::Notify,
    /// Notified when an item is popped (wakes a `Block`-policy `push`
    /// waiting for room).
    notify_pop: tokio::sync::Notify,
    closed: std::sync::atomic::AtomicBool,
}

impl AdaptQueue {
    fn new(capacity: usize, policy: BackPressurePolicy) -> Self {
        Self {
            queue: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(
                capacity.min(256),
            )),
            capacity: capacity.max(1),
            policy,
            notify_push: tokio::sync::Notify::new(),
            notify_pop: tokio::sync::Notify::new(),
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Push `msg` per the configured policy.
    ///
    /// `DropNewest` discards `msg` when full; `DropOldest` evicts the
    /// front of the queue to make room; `Block` waits (async) until there
    /// is room rather than dropping either message.
    async fn push(&self, msg: Message) {
        match self.policy {
            BackPressurePolicy::DropNewest => {
                let mut q = self.queue.lock().unwrap();
                if q.len() < self.capacity {
                    q.push_back(msg);
                    drop(q);
                    self.notify_push.notify_one();
                }
            }
            BackPressurePolicy::DropOldest => {
                let mut q = self.queue.lock().unwrap();
                if q.len() >= self.capacity {
                    q.pop_front();
                }
                q.push_back(msg);
                drop(q);
                self.notify_push.notify_one();
            }
            BackPressurePolicy::Block => loop {
                {
                    let mut q = self.queue.lock().unwrap();
                    if q.len() < self.capacity {
                        q.push_back(msg);
                        drop(q);
                        self.notify_push.notify_one();
                        return;
                    }
                }
                self.notify_pop.notified().await;
            },
        }
    }

    /// Pop the next message, waiting until one is available. Returns `None`
    /// once `close()` has been called and the queue is drained.
    async fn pop(&self) -> Option<Message> {
        loop {
            {
                let mut q = self.queue.lock().unwrap();
                if let Some(m) = q.pop_front() {
                    drop(q);
                    self.notify_pop.notify_one();
                    return Some(m);
                }
            }
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                // One final check in case a push raced with the close.
                return self.queue.lock().unwrap().pop_front();
            }
            self.notify_push.notified().await;
        }
    }

    fn close(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify_push.notify_waiters();
    }
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
        // A malformed msg.id is a structural conversion failure (closer to
        // ErrInvalidFrame territory, §5.3), not a payload-size problem.
        // relay::Node::send() is restricted to the four mandatory sentinels
        // (§10.1), so there is no exact fit; NotConnected is used here as
        // the deliberate "no usable frame could be constructed" fallback,
        // matching the safe-default mapping used below for other
        // unmodelled bus errors — PayloadTooLarge would be actively
        // misleading to a caller matching on it to detect oversized frames.
        let frame = from_message(&msg).map_err(|_| crate::relay::Error::NotConnected)?;
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
    ///
    /// `tokio::sync::mpsc` has no drain-oldest primitive, so the policy
    /// (§10.5 rule 3) is enforced against `AdaptQueue` -- a small buffer this
    /// module owns -- rather than against the mpsc channel's own internal
    /// buffer. The mpsc channel returned to the caller is fed one message at
    /// a time from that queue and exists only to satisfy `Node::subscribe`'s
    /// return type.
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

        let (tx, rx) = mpsc::channel::<Message>(1);
        let queue = Arc::new(AdaptQueue::new(depth, policy));

        // Producer task: converts frames and applies the back-pressure
        // policy against `queue` (§10.5 rule 3).
        let producer_queue = queue.clone();
        tokio::spawn(async move {
            let mut seq: u64 = 0;
            loop {
                match frame_rx.recv().await {
                    None => break,
                    Some(f) => {
                        let mut msg = to_message(&f);
                        msg.seq = seq;
                        seq += 1;
                        producer_queue.push(msg).await;
                    }
                }
            }
            producer_queue.close();
        });

        // Forwarder task: drains `queue` one message at a time into the
        // external channel, pacing itself on the caller's `recv()` calls.
        // §10.5 rule 2: the external channel closes when this task exits
        // (tx is dropped).
        tokio::spawn(async move {
            while let Some(msg) = queue.pop().await {
                if tx.send(msg).await.is_err() {
                    break; // receiver dropped
                }
            }
            // producer closed and queue drained
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

    #[tokio::test]
    async fn adapt_send_invalid_id_is_not_payload_too_large() {
        // §5.3: ErrInvalidFrame/structural failures are distinct from
        // ErrPayloadTooLarge. A malformed msg.id must not surface as
        // PayloadTooLarge, which would mislead a caller checking for an
        // oversized payload.
        use crate::mock::MockBus;
        let mock = Arc::new(MockBus::new());
        let node = adapt(mock);

        let msg = Message::new(Protocol::Can, "not_a_number", vec![]);
        let err = node.send(Context::background(), msg).await.unwrap_err();
        assert_ne!(err, crate::relay::Error::PayloadTooLarge);
    }

    #[tokio::test]
    async fn adapt_subscribe_drop_oldest_delivers_messages() {
        // End-to-end smoke test: DropOldest subscriptions must still wire
        // up and deliver messages (exact eviction ordering under
        // concurrent producer/forwarder scheduling is covered
        // deterministically by `adapt_queue_*` below).
        use crate::mock::MockBus;
        use crate::relay::SubscriberOptions;

        let mock = Arc::new(MockBus::new());
        let node = adapt(mock.clone());

        let mut rx = node
            .subscribe(SubscriberOptions {
                channel_depth: 2,
                back_pressure: BackPressurePolicy::DropOldest,
                rate_limit_per_sec: 0,
            })
            .await
            .unwrap();

        mock.inject(Frame {
            id: 0x123,
            data: vec![0xAB],
            ..Default::default()
        })
        .await;

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.id, "291"); // 0x123
    }

    fn queue_msg(id: &str) -> Message {
        Message::new(Protocol::Can, id, vec![])
    }

    #[tokio::test]
    async fn adapt_queue_drop_oldest_evicts_front() {
        // RELAY spec §10.5 rule 3: DropOldest must evict the oldest
        // buffered message when full, not discard the arriving one
        // (regression test for the bug where DropOldest behaved
        // identically to DropNewest).
        let q = AdaptQueue::new(2, BackPressurePolicy::DropOldest);
        q.push(queue_msg("1")).await;
        q.push(queue_msg("2")).await;
        q.push(queue_msg("3")).await; // queue full — evicts "1"

        assert_eq!(q.pop().await.unwrap().id, "2");
        assert_eq!(q.pop().await.unwrap().id, "3");
    }

    #[tokio::test]
    async fn adapt_queue_drop_newest_discards_arriving() {
        let q = AdaptQueue::new(2, BackPressurePolicy::DropNewest);
        q.push(queue_msg("1")).await;
        q.push(queue_msg("2")).await;
        q.push(queue_msg("3")).await; // queue full — "3" is dropped

        assert_eq!(q.pop().await.unwrap().id, "1");
        assert_eq!(q.pop().await.unwrap().id, "2");
    }

    #[tokio::test]
    async fn adapt_queue_close_drains_then_ends() {
        let q = AdaptQueue::new(2, BackPressurePolicy::DropOldest);
        q.push(queue_msg("1")).await;
        q.close();

        assert_eq!(q.pop().await.unwrap().id, "1");
        assert!(q.pop().await.is_none());
    }
}
