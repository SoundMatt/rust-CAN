// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! In-process virtual CAN bus — no OS dependencies.
//!
//! `VirtualBus` implements Bus, LoaningBus, HealthProvider, MetricsProvider,
//! and Drainer. It broadcasts every sent frame to all matching subscribers.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::bus::{Bus, Drainer, FrameReceiver, HealthProvider, LoaningBus, MetricsProvider, SubInner};
use crate::error::Error;
use crate::frame::{Filter, Frame, LoanedFrame};
use crate::relay::{Context, Health, Metrics, SubscriberOptions};
use crate::validate_frame;

// ---------------------------------------------------------------------------
// Internal subscriber record
// ---------------------------------------------------------------------------

struct VirtualSub {
    filters: Vec<Filter>,
    inner: Arc<SubInner>,
}

impl VirtualSub {
    fn matches(&self, frame: &Frame) -> bool {
        if self.filters.is_empty() {
            return true;
        }
        self.filters.iter().any(|f| f.matches(frame))
    }
}

// ---------------------------------------------------------------------------
// BusInner
// ---------------------------------------------------------------------------

struct BusInner {
    subs: Vec<VirtualSub>,
}

impl BusInner {
    fn new() -> Self {
        Self { subs: Vec::new() }
    }

    /// Broadcast a frame to all matching subscribers.
    ///
    /// Returns (delivered, dropped) counts.
    fn broadcast(&self, frame: &Frame) -> (u64, u64) {
        let mut delivered: u64 = 0;
        let mut dropped: u64 = 0;
        for sub in &self.subs {
            if sub.matches(frame) {
                if sub.inner.push(frame.clone()) {
                    delivered += 1;
                } else {
                    dropped += 1;
                }
            }
        }
        (delivered, dropped)
    }

    /// Remove closed/dead subscriber entries.
    fn gc(&mut self) {
        self.subs
            .retain(|s| !s.inner.closed.load(Ordering::Relaxed));
    }
}

// ---------------------------------------------------------------------------
// VirtualBus
// ---------------------------------------------------------------------------

/// An in-process broadcast CAN bus.
///
/// Every frame sent is delivered to all subscribers whose filters match.
/// Requires no OS or hardware dependencies — suitable for development and testing.
//fusa:req REQ-VIRT-001
pub struct VirtualBus {
    inner: Arc<Mutex<BusInner>>,
    closed: Arc<AtomicBool>,
    // Metrics counters.
    write_count: Arc<AtomicU64>,
    deliver_count: Arc<AtomicU64>,
    drop_count: Arc<AtomicU64>,
    bytes_written: Arc<AtomicU64>,
    bytes_delivered: Arc<AtomicU64>,
    error_count: Arc<AtomicU64>,
}

impl VirtualBus {
    /// Create a new in-process virtual bus.
    //fusa:req REQ-VIRT-001
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BusInner::new())),
            closed: Arc::new(AtomicBool::new(false)),
            write_count: Arc::new(AtomicU64::new(0)),
            deliver_count: Arc::new(AtomicU64::new(0)),
            drop_count: Arc::new(AtomicU64::new(0)),
            bytes_written: Arc::new(AtomicU64::new(0)),
            bytes_delivered: Arc::new(AtomicU64::new(0)),
            error_count: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Default for VirtualBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Bus for VirtualBus {
    /// Broadcast a frame to all matching subscribers.
    //fusa:req REQ-VIRT-002, REQ-VIRT-004
    async fn send(&self, _ctx: Context, frame: Frame) -> Result<(), Error> {
        if self.closed.load(Ordering::SeqCst) {
            self.error_count.fetch_add(1, Ordering::Relaxed);
            return Err(Error::Closed);
        }

        //fusa:req REQ-CAN-004: validate before sending.
        if let Err(e) = validate_frame(&frame) {
            self.error_count.fetch_add(1, Ordering::Relaxed);
            return Err(e);
        }

        let payload_len = frame.data.len() as u64;
        self.bytes_written.fetch_add(payload_len, Ordering::Relaxed);
        self.write_count.fetch_add(1, Ordering::Relaxed);

        let mut guard = self.inner.lock().await;
        guard.gc();
        let (delivered, dropped) = guard.broadcast(&frame);
        drop(guard);

        self.deliver_count
            .fetch_add(delivered, Ordering::Relaxed);
        self.drop_count.fetch_add(dropped, Ordering::Relaxed);
        self.bytes_delivered
            .fetch_add(payload_len * delivered, Ordering::Relaxed);

        Ok(())
    }

    /// Subscribe to frames matching any of the given filters.
    //fusa:req REQ-VIRT-003, REQ-VIRT-004, REQ-VIRT-005
    async fn subscribe(
        &self,
        filters: Vec<Filter>,
        opts: SubscriberOptions,
    ) -> Result<FrameReceiver, Error> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }

        let depth = opts.chan_depth(64);
        let policy = opts.back_pressure;
        let sub_inner = Arc::new(SubInner::new(depth, policy));
        let rx = FrameReceiver {
            inner: sub_inner.clone(),
        };

        let mut guard = self.inner.lock().await;
        guard.subs.push(VirtualSub {
            filters,
            inner: sub_inner,
        });
        drop(guard);

        Ok(rx)
    }

    /// Close the bus. Idempotent per RELAY spec §6.1.
    //fusa:req REQ-CAN-008
    async fn close(&self) -> Result<(), Error> {
        if self
            .closed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Already closed — idempotent.
            return Ok(());
        }

        let mut guard = self.inner.lock().await;
        for sub in &guard.subs {
            sub.inner.close();
        }
        guard.subs.clear();
        drop(guard);

        Ok(())
    }
}

#[async_trait]
impl LoaningBus for VirtualBus {
    /// Allocate a new loaned frame with no-op release.
    async fn loan(&self) -> Result<LoanedFrame, Error> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        Ok(LoanedFrame::simple(Frame::default()))
    }

    /// Send a loaned frame (calls through to Bus::send).
    async fn send_loaned(&self, ctx: Context, frame: LoanedFrame) -> Result<(), Error> {
        let f = frame.frame.clone();
        frame.return_loan();
        self.send(ctx, f).await
    }
}

impl HealthProvider for VirtualBus {
    fn health(&self) -> crate::relay::Health {
        if self.closed.load(Ordering::SeqCst) {
            Health::down("bus is closed")
        } else {
            Health::ok()
        }
    }
}

impl MetricsProvider for VirtualBus {
    fn metrics(&self) -> Metrics {
        Metrics {
            write_count: self.write_count.load(Ordering::Relaxed),
            deliver_count: self.deliver_count.load(Ordering::Relaxed),
            drop_count: self.drop_count.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            bytes_delivered: self.bytes_delivered.load(Ordering::Relaxed),
            error_count: self.error_count.load(Ordering::Relaxed),
        }
    }
}

#[async_trait]
impl Drainer for VirtualBus {
    /// Close after draining all subscriber queues, or until ctx expires.
    ///
    /// Per RELAY spec §9.2: if draining completes before the deadline,
    /// returns Ok(()); if ctx expires first, returns Err(Timeout).
    async fn close_with_drain(&self, ctx: Context) -> Result<(), Error> {
        // Poll subscriber queues until all are empty or ctx expires.
        loop {
            if ctx.done() {
                // Mark as closed, dropping undelivered messages.
                let _ = self.close().await;
                return Err(Error::Timeout);
            }

            let guard = self.inner.lock().await;
            let all_empty = guard.subs.iter().all(|s| s.inner.is_empty());
            drop(guard);

            if all_empty {
                break;
            }

            sleep(Duration::from_millis(1)).await;
        }

        self.close().await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::BackPressurePolicy;

    fn make_frame(id: u32, data: Vec<u8>) -> Frame {
        Frame {
            id,
            data,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn send_and_receive() {
        let bus = VirtualBus::new();
        let rx = bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();

        bus.send(Context::background(), make_frame(0x100, vec![1, 2, 3]))
            .await
            .unwrap();

        let f = rx.recv().await.unwrap();
        assert_eq!(f.id, 0x100);
        assert_eq!(f.data, vec![1, 2, 3]);
    }

    //fusa:req REQ-VIRT-002
    #[tokio::test]
    async fn broadcast_to_multiple_subscribers() {
        let bus = VirtualBus::new();
        let rx1 = bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();
        let rx2 = bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();

        bus.send(Context::background(), make_frame(0x200, vec![]))
            .await
            .unwrap();

        assert_eq!(rx1.recv().await.unwrap().id, 0x200);
        assert_eq!(rx2.recv().await.unwrap().id, 0x200);
    }

    //fusa:req REQ-VIRT-004
    #[tokio::test]
    async fn filter_semantics() {
        let bus = VirtualBus::new();
        let filter = vec![Filter { id: 0x100, mask: 0x7FF }];
        let rx = bus
            .subscribe(filter, SubscriberOptions::default())
            .await
            .unwrap();

        // This frame should NOT match.
        bus.send(Context::background(), make_frame(0x200, vec![]))
            .await
            .unwrap();

        // This frame SHOULD match.
        bus.send(Context::background(), make_frame(0x100, vec![1]))
            .await
            .unwrap();

        let f = rx.recv().await.unwrap();
        assert_eq!(f.id, 0x100);
    }

    //fusa:req REQ-CAN-008
    #[tokio::test]
    async fn send_after_close_returns_closed() {
        let bus = VirtualBus::new();
        bus.close().await.unwrap();
        let err = bus
            .send(Context::background(), make_frame(0x100, vec![]))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Closed));
    }

    #[tokio::test]
    async fn close_is_idempotent() {
        let bus = VirtualBus::new();
        bus.close().await.unwrap();
        bus.close().await.unwrap(); // second close must not error
    }

    //fusa:req REQ-VIRT-005
    #[tokio::test]
    async fn back_pressure_drop_newest() {
        let bus = VirtualBus::new();
        let rx = bus
            .subscribe(
                vec![],
                SubscriberOptions {
                    channel_depth: 2,
                    back_pressure: BackPressurePolicy::DropNewest,
                },
            )
            .await
            .unwrap();

        for i in 0..5u32 {
            bus.send(Context::background(), make_frame(i, vec![]))
                .await
                .unwrap();
        }

        let f1 = rx.recv().await.unwrap();
        let f2 = rx.recv().await.unwrap();
        // Only the first two frames fit.
        assert_eq!(f1.id, 0);
        assert_eq!(f2.id, 1);
    }

    #[tokio::test]
    async fn metrics_tracking() {
        let bus = VirtualBus::new();
        let _rx = bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();

        bus.send(Context::background(), make_frame(0x100, vec![1, 2, 3, 4]))
            .await
            .unwrap();

        let m = bus.metrics();
        assert_eq!(m.write_count, 1);
        assert_eq!(m.deliver_count, 1);
        assert_eq!(m.bytes_written, 4);
        assert_eq!(m.bytes_delivered, 4);
    }

    #[tokio::test]
    async fn health_closed_vs_open() {
        let bus = VirtualBus::new();
        assert_eq!(bus.health().status, crate::relay::HealthStatus::Ok);
        bus.close().await.unwrap();
        assert_eq!(bus.health().status, crate::relay::HealthStatus::Down);
    }

    #[tokio::test]
    async fn loaning_bus_roundtrip() {
        let bus = VirtualBus::new();
        let rx = bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();

        let mut lf = bus.loan().await.unwrap();
        lf.frame.id = 0x300;
        lf.frame.data = vec![0xAB];

        bus.send_loaned(Context::background(), lf).await.unwrap();
        let f = rx.recv().await.unwrap();
        assert_eq!(f.id, 0x300);
    }

    #[tokio::test]
    async fn invalid_frame_rejected() {
        let bus = VirtualBus::new();
        let bad_frame = Frame {
            id: 0xFFF, // exceeds 0x7FF for standard CAN
            ..Default::default()
        };
        let err = bus
            .send(Context::background(), bad_frame)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::InvalidFrame { .. }));
    }
}
