// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! rust-CAN — CAN bus library for Rust.
//!
//! Provides a virtual bus, SocketCAN (Linux), DBC parser, ISO-TP, J1939,
//! CAN FD, and safety E2E protection. Conforms to RELAY spec v1.1.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use rust_can::{virtual_bus::VirtualBus, Bus, Frame};
//! use rust_can::relay::Context;
//! use rust_can::relay::SubscriberOptions;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let bus = Arc::new(VirtualBus::new());
//!
//!     let rx = bus.subscribe(vec![], SubscriberOptions::default()).await.unwrap();
//!
//!     bus.send(Context::background(), Frame {
//!         id: 0x100,
//!         data: vec![0xDE, 0xAD, 0xBE, 0xEF],
//!         ..Default::default()
//!     }).await.unwrap();
//!
//!     let frame = rx.recv().await.unwrap();
//!     println!("Received frame: id=0x{:X} data={:?}", frame.id, frame.data);
//!
//!     bus.close().await.unwrap();
//! }
//! ```

pub mod relay;
pub mod error;
pub mod frame;
pub mod bus;
pub mod adapt;
pub mod virtual_bus;
pub mod mock;
pub mod isotp;
pub mod j1939;
pub mod dbc;
pub mod safety;
pub(crate) mod crc;

#[cfg(target_os = "linux")]
pub mod socketcan;

pub use error::Error;
pub use frame::{
    validate_frame, max_data_len, Frame, Filter, LoanedFrame,
    CAN_MAX_DATA_LEN, CAN_FD_MAX_DATA_LEN, CAN_XL_MIN_DATA_LEN, CAN_XL_MAX_DATA_LEN,
    CAN_MAX_STD_ID, CAN_MAX_EXT_ID, CAN_XL_MAX_PRIO_ID,
};
pub use bus::{Bus, LoaningBus, HealthProvider, MetricsProvider, Drainer, FrameReceiver};
pub use adapt::{to_message, from_message, adapt};

/// The RELAY spec version this implementation targets.
pub const SPEC_VERSION: &str = "1.1";

/// Alias for `SPEC_VERSION` for explicitness in CLI contexts.
pub const RELAY_SPEC_VERSION: &str = "1.1";
