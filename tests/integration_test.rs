// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Integration tests for rust-CAN.
//!
//! Every test is annotated with `//fusa:req` so that rsfusa verify can trace
//! it to the requirement it verifies.

use std::sync::Arc;

use rust_can::relay::{Context, Protocol, SubscriberOptions};
use rust_can::virtual_bus::VirtualBus;
use rust_can::{adapt, from_message, to_message, Bus, Filter, Frame};

// ---------------------------------------------------------------------------
// Virtual bus integration
// ---------------------------------------------------------------------------

//fusa:test REQ-VIRT-001, REQ-VIRT-002
#[tokio::test]
async fn virtual_bus_send_receive_roundtrip() {
    let bus = Arc::new(VirtualBus::new());
    let rx = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .unwrap();

    let sent = Frame {
        id: 0x123,
        ext: false,
        data: vec![0xDE, 0xAD, 0xBE, 0xEF],
        ..Default::default()
    };

    bus.send(Context::background(), sent.clone()).await.unwrap();

    let received = rx.recv().await.unwrap();
    assert_eq!(received.id, sent.id);
    assert_eq!(received.data, sent.data);
}

//fusa:test REQ-VIRT-002, REQ-VIRT-003
#[tokio::test]
async fn virtual_bus_multiple_subscribers_all_receive() {
    let bus = Arc::new(VirtualBus::new());
    let rx1 = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .unwrap();
    let rx2 = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .unwrap();
    let rx3 = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .unwrap();

    bus.send(
        Context::background(),
        Frame {
            id: 0x456,
            data: vec![1],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    for rx in [&rx1, &rx2, &rx3] {
        let f = rx.recv().await.unwrap();
        assert_eq!(f.id, 0x456);
    }
}

//fusa:test REQ-VIRT-004
#[tokio::test]
async fn virtual_bus_filter_precision() {
    let bus = Arc::new(VirtualBus::new());

    let rx_all = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .unwrap();
    let rx_100 = bus
        .subscribe(
            vec![Filter {
                id: 0x100,
                mask: 0x7FF,
            }],
            SubscriberOptions::default(),
        )
        .await
        .unwrap();

    bus.send(
        Context::background(),
        Frame {
            id: 0x100,
            data: vec![1],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    bus.send(
        Context::background(),
        Frame {
            id: 0x200,
            data: vec![2],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // rx_all gets both.
    let f1 = rx_all.recv().await.unwrap();
    let f2 = rx_all.recv().await.unwrap();
    assert_eq!(f1.id, 0x100);
    assert_eq!(f2.id, 0x200);

    // rx_100 gets only 0x100.
    let f = rx_100.recv().await.unwrap();
    assert_eq!(f.id, 0x100);
}

// ---------------------------------------------------------------------------
// Lifecycle invariants
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-008
#[tokio::test]
async fn close_then_send_returns_closed() {
    let bus = Arc::new(VirtualBus::new());
    bus.close().await.unwrap();
    let err = bus
        .send(
            Context::background(),
            Frame {
                id: 0x100,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, rust_can::Error::Closed));
}

//fusa:test REQ-CAN-008
#[tokio::test]
async fn close_then_subscribe_returns_closed() {
    let bus = Arc::new(VirtualBus::new());
    bus.close().await.unwrap();
    let err = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .expect_err("expected Closed error");
    assert!(matches!(err, rust_can::Error::Closed));
}

//fusa:test REQ-CAN-008
#[tokio::test]
async fn close_is_idempotent() {
    let bus = Arc::new(VirtualBus::new());
    for _ in 0..5 {
        bus.close().await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// RELAY adapter
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-007
#[tokio::test]
async fn adapt_send_and_receive_via_relay_node() {
    use rust_can::relay::Message;

    let bus = Arc::new(VirtualBus::new());
    let frame_rx = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .unwrap();

    let node = adapt(bus.clone());
    let ctx = Context::background();

    let msg = Message::new(Protocol::Can, "256", vec![0x01, 0x02]);
    node.send(ctx, msg).await.unwrap();

    let f = frame_rx.recv().await.unwrap();
    assert_eq!(f.id, 256);
    assert_eq!(f.data, vec![0x01, 0x02]);
}

//fusa:test REQ-CAN-007
#[tokio::test]
async fn to_message_from_message_roundtrip() {
    let original = Frame {
        id: 0x7FF,
        ext: false,
        fd: true,
        brs: true,
        data: vec![0xAA, 0xBB, 0xCC],
        ..Default::default()
    };

    let msg = to_message(&original);
    assert_eq!(msg.protocol, Protocol::Can);
    assert_eq!(msg.id, "2047"); // 0x7FF = 2047

    let recovered = from_message(&msg).unwrap();
    assert_eq!(recovered.id, original.id);
    assert_eq!(recovered.fd, original.fd);
    assert_eq!(recovered.brs, original.brs);
    assert_eq!(recovered.data, original.data);
}

// ---------------------------------------------------------------------------
// Frame validation
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-004, REQ-CAN-009
#[test]
fn validate_frame_standard_id_boundary() {
    use rust_can::validate_frame;
    assert!(validate_frame(&Frame {
        id: 0x7FF,
        ..Default::default()
    })
    .is_ok());
    assert!(validate_frame(&Frame {
        id: 0x800,
        ..Default::default()
    })
    .is_err());
}

//fusa:test REQ-CAN-004, REQ-CAN-010
#[test]
fn validate_frame_extended_id_boundary() {
    use rust_can::validate_frame;
    assert!(validate_frame(&Frame {
        id: 0x1FFF_FFFF,
        ext: true,
        ..Default::default()
    })
    .is_ok());
    assert!(validate_frame(&Frame {
        id: 0x2000_0000,
        ext: true,
        ..Default::default()
    })
    .is_err());
}

//fusa:test REQ-CAN-004, REQ-CAN-014
#[test]
fn validate_frame_fd_xl_mutual_exclusion() {
    use rust_can::validate_frame;
    let f = Frame {
        id: 0x100,
        fd: true,
        xl: true,
        data: vec![0],
        ..Default::default()
    };
    assert!(validate_frame(&f).is_err());
}

// ---------------------------------------------------------------------------
// Mock bus
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-003, REQ-CAN-006
#[tokio::test]
async fn mock_bus_records_and_injects() {
    use rust_can::mock::MockBus;

    let bus = MockBus::new();
    let rx = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .unwrap();

    bus.send(
        Context::background(),
        Frame {
            id: 0x100,
            data: vec![42],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let sent = bus.sent_frames().await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].id, 0x100);

    bus.inject(Frame {
        id: 0x200,
        data: vec![99],
        ..Default::default()
    })
    .await;
    let f = rx.recv().await.unwrap();
    assert_eq!(f.id, 0x200);
    assert_eq!(f.data, vec![99]);
}

// ---------------------------------------------------------------------------
// Safety E2E
// ---------------------------------------------------------------------------

//fusa:test REQ-SAFETY-001, REQ-SAFETY-002, REQ-SAFETY-003, REQ-SAFETY-004
#[test]
fn safety_protect_unwrap_roundtrip() {
    use rust_can::safety::{Config, Protector, Receiver};

    let cfg = Config {
        data_id: 0x0001,
        source_id: 0x0002,
    };
    let protector = Protector::new(cfg);
    let receiver = Receiver::new(cfg);

    let payload = b"test payload for safety";
    let protected = protector.protect(payload);
    assert_eq!(protected.len(), payload.len() + 10);

    let recovered = receiver.unwrap(&protected).unwrap();
    assert_eq!(recovered, payload);
}

//fusa:test REQ-SAFETY-002, REQ-SAFETY-004
#[test]
fn safety_crc_mismatch_detected() {
    use rust_can::safety::{Config, E2EErrorKind, Protector, Receiver};

    let cfg = Config {
        data_id: 0x0001,
        source_id: 0x0002,
    };
    let protector = Protector::new(cfg);
    let receiver = Receiver::new(cfg);

    let mut protected = protector.protect(b"data");
    protected[10] ^= 0xFF; // corrupt payload byte

    let err = receiver.unwrap(&protected).unwrap_err();
    assert_eq!(err.kind, E2EErrorKind::CrcMismatch);
}

// ---------------------------------------------------------------------------
// J1939
// ---------------------------------------------------------------------------

//fusa:test REQ-J1939-001, REQ-J1939-004
#[test]
fn j1939_encode_decode_roundtrip() {
    use rust_can::j1939::{decode_id, encode_id, Pgn, Priority, BROADCAST_ADDR};

    let priority = Priority(3);
    let pgn = Pgn(0x0FEF1); // broadcast, PF=0xFE ≥ 240
    let src = 0x10u8;
    let dst = BROADCAST_ADDR;

    let id = encode_id(priority, pgn, src, dst);
    let (p, g, s) = decode_id(id);

    assert_eq!(p.value(), 3);
    assert_eq!(g, pgn);
    assert_eq!(s, src);
}

// ---------------------------------------------------------------------------
// DBC
// ---------------------------------------------------------------------------

//fusa:test REQ-DBC-001, REQ-DBC-002
#[test]
fn dbc_parse_and_decode() {
    use rust_can::dbc::parse;

    let dbc_src = r#"
BO_ 100 SpeedMsg: 4 ECU
 SG_ Speed : 0|16@1+ (0.1,0) [0|6553.5] "kph" Vector__XXX

"#;
    let db = parse(dbc_src).unwrap();
    let msg = db.messages.get(&100).unwrap();
    assert_eq!(msg.name, "SpeedMsg");
    assert_eq!(msg.signals.len(), 1);
    assert_eq!(msg.signals[0].name, "Speed");

    // data = [0xE8, 0x03] → raw = 0x03E8 = 1000 → 1000 * 0.1 = 100.0 kph
    let data = vec![0xE8u8, 0x03, 0, 0];
    let values = db.decode(100, &data);
    let speed = values["Speed"];
    assert!((speed - 100.0).abs() < 0.01, "speed={}", speed);
}

// ---------------------------------------------------------------------------
// ISO-TP
// ---------------------------------------------------------------------------

//fusa:test REQ-ISOTP-001, REQ-ISOTP-002, REQ-ISOTP-004
#[tokio::test]
async fn isotp_single_frame_roundtrip() {
    use rust_can::isotp::{Config, IsoTpConn};

    let bus = Arc::new(VirtualBus::new());
    let sender_cfg = Config {
        tx_id: 0x7E0,
        rx_id: 0x7E8,
        timeout: std::time::Duration::from_millis(200),
        ..Default::default()
    };
    let receiver_cfg = Config {
        tx_id: 0x7E8,
        rx_id: 0x7E0,
        timeout: std::time::Duration::from_millis(200),
        ..Default::default()
    };

    let sender = IsoTpConn::new(bus.clone(), sender_cfg).await.unwrap();
    let receiver = IsoTpConn::new(bus.clone(), receiver_cfg).await.unwrap();

    let payload = b"hello!!"; // exactly 7 bytes — single frame

    let recv_handle = tokio::spawn(async move { receiver.recv(Context::background()).await });

    sender.send(Context::background(), payload).await.unwrap();

    let result = recv_handle.await.unwrap().unwrap();
    assert_eq!(result, payload);
}

// ---------------------------------------------------------------------------
// Spec version
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-001
#[test]
fn spec_version_constant() {
    assert_eq!(rust_can::SPEC_VERSION, "1.1");
    assert_eq!(rust_can::RELAY_SPEC_VERSION, "1.1");
}
