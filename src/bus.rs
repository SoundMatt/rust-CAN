// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Core Bus traits and the FrameReceiver subscriber type.
//!
//! This module defines the primary interface contract per RELAY spec §8.1.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::Notify;

use crate::error::Error;
use crate::frame::{Filter, Frame, LoanedFrame};
use crate::relay::{BackPressurePolicy, Context, Health, Metrics, SubscriberOptions};

// ---------------------------------------------------------------------------
// SubInner — shared subscriber queue
// ---------------------------------------------------------------------------

/// Inner shared state for a subscriber channel.
///
/// Uses `std::sync::Mutex` for the queue so it can be locked briefly from
/// both sync and async contexts without holding across await points.
pub(crate) struct SubInner {
    pub(crate) queue: Mutex<VecDeque<Frame>>,
    pub(crate) capacity: usize,
    pub(crate) policy: BackPressurePolicy,
    pub(crate) notify: Notify,
    pub(crate) closed: AtomicBool,
}

impl SubInner {
    pub(crate) fn new(capacity: usize, policy: BackPressurePolicy) -> Self {
        Self {
            queue: Mutex::new(VecDeque::with_capacity(capacity.min(256))),
            capacity,
            policy,
            notify: Notify::new(),
            closed: AtomicBool::new(false),
        }
    }

    /// Push a frame into the queue, applying the back-pressure policy.
    ///
    /// Returns `true` if the frame was accepted (delivered), `false` if it
    /// was dropped.
    pub(crate) fn push(&self, frame: Frame) -> bool {
        let mut q = self.queue.lock().unwrap();
        match self.policy {
            BackPressurePolicy::DropNewest => {
                if q.len() >= self.capacity {
                    return false;
                }
                q.push_back(frame);
                self.notify.notify_one();
                true
            }
            BackPressurePolicy::DropOldest => {
                if q.len() >= self.capacity {
                    q.pop_front();
                }
                q.push_back(frame);
                self.notify.notify_one();
                true
            }
            BackPressurePolicy::Block => {
                // For virtual/mock buses: always push (capacity is advisory
                // for Block policy). In production, the caller should drain
                // fast enough.
                q.push_back(frame);
                self.notify.notify_one();
                true
            }
        }
    }

    /// Pop the front frame from the queue.
    pub(crate) fn pop(&self) -> Option<Frame> {
        self.queue.lock().unwrap().pop_front()
    }

    /// Returns true if the queue is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.queue.lock().unwrap().is_empty()
    }

    /// Close this subscriber channel, unblocking any waiting receivers.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

// ---------------------------------------------------------------------------
// FrameReceiver
// ---------------------------------------------------------------------------

/// The receiving end of a CAN subscriber channel.
///
/// Created by `Bus::subscribe`. Call `recv()` in a loop to consume frames.
pub struct FrameReceiver {
    pub(crate) inner: std::sync::Arc<SubInner>,
}

impl std::fmt::Debug for FrameReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameReceiver")
            .field("closed", &self.inner.closed.load(Ordering::Relaxed))
            .finish()
    }
}

impl FrameReceiver {
    /// Receive the next frame, waiting until one is available.
    ///
    /// Returns `None` when the bus is closed and the queue is drained.
    pub async fn recv(&self) -> Option<Frame> {
        loop {
            // Try to pop a frame from the queue.
            if let Some(f) = self.inner.pop() {
                return Some(f);
            }
            // If closed and queue is empty, signal end of stream.
            if self.inner.closed.load(Ordering::SeqCst) {
                // One final check in case a frame arrived between pop and load.
                return self.inner.pop();
            }
            // Wait for a notification.
            self.inner.notify.notified().await;
        }
    }

    /// Close this receiver. The bus will stop delivering frames to it.
    pub fn close(&self) {
        self.inner.close();
    }
}

// ---------------------------------------------------------------------------
// Bus trait
// ---------------------------------------------------------------------------

/// The primary CAN bus interface per RELAY spec §8.1.
///
/// Implementations must be safe for concurrent use from multiple async tasks.
//fusa:req REQ-CAN-003, REQ-CAN-006
#[async_trait]
pub trait Bus: Send + Sync {
    /// Transmit a CAN frame. Blocks until the frame is accepted or ctx expires.
    async fn send(&self, ctx: Context, frame: Frame) -> Result<(), Error>;

    /// Subscribe to frames that match any of the given filters.
    ///
    /// An empty `filters` slice means "accept all frames" (no filtering).
    async fn subscribe(
        &self,
        filters: Vec<Filter>,
        opts: SubscriberOptions,
    ) -> Result<FrameReceiver, Error>;

    /// Close the bus and all subscriber channels. Idempotent.
    //fusa:req REQ-CAN-008
    async fn close(&self) -> Result<(), Error>;
}

// ---------------------------------------------------------------------------
// LoaningBus trait
// ---------------------------------------------------------------------------

/// Optional zero-copy extension per RELAY spec §8.1.
//fusa:req REQ-CAN-003
#[async_trait]
pub trait LoaningBus: Bus {
    /// Acquire a loaned frame from the bus pool.
    async fn loan(&self) -> Result<LoanedFrame, Error>;

    /// Transmit a previously loaned frame.
    async fn send_loaned(&self, ctx: Context, frame: LoanedFrame) -> Result<(), Error>;
}

// ---------------------------------------------------------------------------
// Optional interfaces
// ---------------------------------------------------------------------------

/// Optional health reporting interface per RELAY spec §9.
pub trait HealthProvider {
    fn health(&self) -> Health;
}

/// Optional metrics reporting interface per RELAY spec §9.1.
pub trait MetricsProvider {
    fn metrics(&self) -> Metrics;
}

/// Optional graceful shutdown with drain per RELAY spec §9.2.
#[async_trait]
pub trait Drainer {
    /// Close after draining all in-flight messages or until ctx expires.
    async fn close_with_drain(&self, ctx: Context) -> Result<(), Error>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn sub_inner_push_pop() {
        let inner = SubInner::new(4, BackPressurePolicy::DropNewest);
        let f = Frame {
            id: 0x100,
            data: vec![1, 2],
            ..Default::default()
        };
        assert!(inner.push(f.clone()));
        let got = inner.pop().unwrap();
        assert_eq!(got.id, 0x100);
    }

    #[tokio::test]
    async fn sub_inner_drop_newest() {
        let inner = SubInner::new(2, BackPressurePolicy::DropNewest);
        let f1 = Frame { id: 1, ..Default::default() };
        let f2 = Frame { id: 2, ..Default::default() };
        let f3 = Frame { id: 3, ..Default::default() };
        assert!(inner.push(f1));
        assert!(inner.push(f2));
        // Queue full — drop newest (incoming).
        assert!(!inner.push(f3));
        // First two remain.
        assert_eq!(inner.pop().unwrap().id, 1);
        assert_eq!(inner.pop().unwrap().id, 2);
        assert!(inner.pop().is_none());
    }

    #[tokio::test]
    async fn sub_inner_drop_oldest() {
        let inner = SubInner::new(2, BackPressurePolicy::DropOldest);
        let f1 = Frame { id: 1, ..Default::default() };
        let f2 = Frame { id: 2, ..Default::default() };
        let f3 = Frame { id: 3, ..Default::default() };
        inner.push(f1);
        inner.push(f2);
        // Queue full — drop oldest (f1).
        inner.push(f3);
        assert_eq!(inner.pop().unwrap().id, 2);
        assert_eq!(inner.pop().unwrap().id, 3);
        assert!(inner.pop().is_none());
    }

    #[tokio::test]
    async fn frame_receiver_recv_and_close() {
        let inner = Arc::new(SubInner::new(4, BackPressurePolicy::DropNewest));
        let rx = FrameReceiver { inner: inner.clone() };

        let f = Frame { id: 0x200, ..Default::default() };
        inner.push(f);
        inner.close();

        let got = rx.recv().await.unwrap();
        assert_eq!(got.id, 0x200);
        // After drain, returns None.
        assert!(rx.recv().await.is_none());
    }
}
