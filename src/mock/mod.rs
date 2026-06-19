// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! MockBus — an in-memory CAN bus for unit testing.
//!
//! Records all sent frames and allows injecting frames to subscribers.
//! Provides no real broadcast — use VirtualBus for full broadcast semantics.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::bus::{Bus, FrameReceiver, SubInner};
use crate::error::Error;
use crate::frame::{Filter, Frame};
use crate::relay::{Context, SubscriberOptions};

// ---------------------------------------------------------------------------
// MockBus
// ---------------------------------------------------------------------------

/// A mock CAN bus for unit tests.
///
/// Sent frames are recorded and retrievable via `sent_frames()`.
/// Inject frames to all active subscribers with `inject()`.
pub struct MockBus {
    sent: Arc<Mutex<Vec<Frame>>>,
    closed: Arc<AtomicBool>,
    subscribers: Arc<Mutex<Vec<Arc<SubInner>>>>,
}

impl MockBus {
    /// Create a new empty mock bus.
    pub fn new() -> Self {
        Self {
            sent: Arc::new(Mutex::new(Vec::new())),
            closed: Arc::new(AtomicBool::new(false)),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Inject a frame to all active subscribers.
    pub async fn inject(&self, frame: Frame) {
        let subs = self.subscribers.lock().await;
        for sub in subs.iter() {
            if !sub.closed.load(Ordering::Relaxed) {
                sub.push(frame.clone());
            }
        }
    }

    /// Return a copy of all frames that have been sent via `Bus::send`.
    pub async fn sent_frames(&self) -> Vec<Frame> {
        self.sent.lock().await.clone()
    }

    /// Clear the sent-frames log.
    pub async fn reset(&self) {
        self.sent.lock().await.clear();
    }
}

impl Default for MockBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Bus for MockBus {
    async fn send(&self, _ctx: Context, frame: Frame) -> Result<(), Error> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        self.sent.lock().await.push(frame);
        Ok(())
    }

    async fn subscribe(
        &self,
        _filters: Vec<Filter>,
        opts: SubscriberOptions,
    ) -> Result<FrameReceiver, Error> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }

        let depth = opts.chan_depth(64);
        let sub_inner = Arc::new(SubInner::new(depth, opts.back_pressure, opts.rate_limit_per_sec));
        let rx = FrameReceiver {
            inner: sub_inner.clone(),
        };
        self.subscribers.lock().await.push(sub_inner);
        Ok(rx)
    }

    async fn close(&self) -> Result<(), Error> {
        if self
            .closed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(());
        }
        let subs = self.subscribers.lock().await;
        for sub in subs.iter() {
            sub.close();
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::SubscriberOptions;

    fn make_frame(id: u32) -> Frame {
        Frame {
            id,
            data: vec![id as u8],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn records_sent_frames() {
        let bus = MockBus::new();
        bus.send(Context::background(), make_frame(0x100))
            .await
            .unwrap();
        bus.send(Context::background(), make_frame(0x200))
            .await
            .unwrap();
        let sent = bus.sent_frames().await;
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].id, 0x100);
        assert_eq!(sent[1].id, 0x200);
    }

    #[tokio::test]
    async fn reset_clears_sent() {
        let bus = MockBus::new();
        bus.send(Context::background(), make_frame(0x100))
            .await
            .unwrap();
        bus.reset().await;
        assert!(bus.sent_frames().await.is_empty());
    }

    #[tokio::test]
    async fn inject_delivers_to_subscribers() {
        let bus = MockBus::new();
        let rx = bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();

        bus.inject(make_frame(0x300)).await;
        let f = rx.recv().await.unwrap();
        assert_eq!(f.id, 0x300);
    }

    #[tokio::test]
    async fn inject_delivers_to_multiple_subscribers() {
        let bus = MockBus::new();
        let rx1 = bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();
        let rx2 = bus
            .subscribe(vec![], SubscriberOptions::default())
            .await
            .unwrap();

        bus.inject(make_frame(0x400)).await;
        assert_eq!(rx1.recv().await.unwrap().id, 0x400);
        assert_eq!(rx2.recv().await.unwrap().id, 0x400);
    }

    #[tokio::test]
    async fn send_after_close_returns_error() {
        let bus = MockBus::new();
        bus.close().await.unwrap();
        let err = bus
            .send(Context::background(), make_frame(0x100))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Closed));
    }

    #[tokio::test]
    async fn close_is_idempotent() {
        let bus = MockBus::new();
        bus.close().await.unwrap();
        bus.close().await.unwrap();
    }
}
