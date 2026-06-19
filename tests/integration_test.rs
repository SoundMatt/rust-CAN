// Copyright (c) 2026 Matt Jones. All rights reserved.
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Integration tests for rust-CAN.
//!
//! Every test is annotated with `//fusa:test` so that rsfusa verify can trace
//! it to the requirement it verifies.

use std::sync::Arc;

use rust_can::relay::{BackPressurePolicy, Context, Protocol, SubscriberOptions};
use rust_can::virtual_bus::VirtualBus;
use rust_can::{adapt, from_message, to_message, Bus, Filter, Frame};

// ---------------------------------------------------------------------------
// Virtual bus integration
// ---------------------------------------------------------------------------

//fusa:test REQ-VIRT-001
//fusa:test REQ-VIRT-002
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

//fusa:test REQ-VIRT-002
//fusa:test REQ-VIRT-003
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

//fusa:test REQ-CAN-004
//fusa:test REQ-CAN-009
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

//fusa:test REQ-CAN-004
//fusa:test REQ-CAN-010
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

//fusa:test REQ-CAN-004
//fusa:test REQ-CAN-014
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

//fusa:test REQ-CAN-003
//fusa:test REQ-CAN-006
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

//fusa:test REQ-SAFETY-001
//fusa:test REQ-SAFETY-002
//fusa:test REQ-SAFETY-003
//fusa:test REQ-SAFETY-004
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

//fusa:test REQ-SAFETY-002
//fusa:test REQ-SAFETY-004
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

//fusa:test REQ-J1939-001
//fusa:test REQ-J1939-004
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

//fusa:test REQ-DBC-001
//fusa:test REQ-DBC-002
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

//fusa:test REQ-ISOTP-001
//fusa:test REQ-ISOTP-002
//fusa:test REQ-ISOTP-004
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

// ---------------------------------------------------------------------------
// Concurrent safety (REQ-CAN-006)
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-006
#[tokio::test]
async fn concurrent_senders_no_panic() {
    use tokio::task::JoinSet;

    let bus = Arc::new(VirtualBus::new());
    let rx = bus
        .subscribe(vec![], SubscriberOptions::default())
        .await
        .unwrap();

    let mut set = JoinSet::new();
    for i in 0u32..8 {
        let b = bus.clone();
        set.spawn(async move {
            for j in 0u32..16 {
                b.send(
                    Context::background(),
                    Frame {
                        id: (i * 16 + j) & 0x7FF,
                        data: vec![i as u8, j as u8],
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            }
        });
    }
    set.join_all().await;
    drop(rx);
}

// ---------------------------------------------------------------------------
// Frame validation boundaries (REQ-CAN-009..REQ-CAN-013)
// ---------------------------------------------------------------------------

//fusa:test REQ-CAN-011
#[test]
fn validate_frame_brs_requires_fd() {
    use rust_can::validate_frame;
    assert!(validate_frame(&Frame {
        id: 0x100,
        brs: true,
        fd: false,
        ..Default::default()
    })
    .is_err());
    assert!(validate_frame(&Frame {
        id: 0x100,
        brs: true,
        fd: true,
        ..Default::default()
    })
    .is_ok());
}

//fusa:test REQ-CAN-012
#[test]
fn validate_frame_rtr_rejected_on_fd() {
    use rust_can::validate_frame;
    assert!(validate_frame(&Frame {
        id: 0x100,
        fd: true,
        rtr: true,
        ..Default::default()
    })
    .is_err());
}

//fusa:test REQ-CAN-013
#[test]
fn validate_frame_data_length_limits() {
    use rust_can::validate_frame;
    // Classic: max 8 bytes
    assert!(validate_frame(&Frame {
        id: 0x100,
        data: vec![0u8; 8],
        ..Default::default()
    })
    .is_ok());
    assert!(validate_frame(&Frame {
        id: 0x100,
        data: vec![0u8; 9],
        ..Default::default()
    })
    .is_err());
    // FD: max 64 bytes
    assert!(validate_frame(&Frame {
        id: 0x100,
        fd: true,
        data: vec![0u8; 64],
        ..Default::default()
    })
    .is_ok());
    assert!(validate_frame(&Frame {
        id: 0x100,
        fd: true,
        data: vec![0u8; 65],
        ..Default::default()
    })
    .is_err());
}

// ---------------------------------------------------------------------------
// Safety CRC known-good vector (REQ-SAFETY-002)
// ---------------------------------------------------------------------------

//fusa:test REQ-SAFETY-002
#[test]
fn safety_crc_known_vector() {
    use rust_can::safety::{Config, Protector, Receiver};

    let cfg = Config {
        data_id: 0x0000,
        source_id: 0x0000,
    };
    let protector = Protector::new(cfg);
    let receiver = Receiver::new(cfg);

    let payload = b"";
    let protected = protector.protect(payload);
    assert_eq!(protected.len(), 10);
    receiver
        .unwrap(&protected)
        .expect("known-good vector must verify");
}

// ---------------------------------------------------------------------------
// ISO-TP multi-frame (REQ-ISOTP-002, REQ-ISOTP-003)
// ---------------------------------------------------------------------------

//fusa:test REQ-ISOTP-002
//fusa:test REQ-ISOTP-003
#[tokio::test]
async fn isotp_multi_frame_roundtrip() {
    use rust_can::isotp::{Config, IsoTpConn};

    let bus = Arc::new(VirtualBus::new());
    let sender_cfg = Config {
        tx_id: 0x7E0,
        rx_id: 0x7E8,
        timeout: std::time::Duration::from_millis(500),
        ..Default::default()
    };
    let receiver_cfg = Config {
        tx_id: 0x7E8,
        rx_id: 0x7E0,
        timeout: std::time::Duration::from_millis(500),
        ..Default::default()
    };

    let sender = IsoTpConn::new(bus.clone(), sender_cfg).await.unwrap();
    let receiver = IsoTpConn::new(bus.clone(), receiver_cfg).await.unwrap();

    let payload: Vec<u8> = (0u8..=99).collect(); // 100 bytes — multi-frame
    let payload_clone = payload.clone();

    let recv_handle = tokio::spawn(async move { receiver.recv(Context::background()).await });

    sender.send(Context::background(), &payload).await.unwrap();

    let result = recv_handle.await.unwrap().unwrap();
    assert_eq!(result, payload_clone);
}

// ---------------------------------------------------------------------------
// Back-pressure policies (REQ-VIRT-005)
// ---------------------------------------------------------------------------

//fusa:test REQ-VIRT-005
#[tokio::test]
async fn back_pressure_drop_oldest() {
    use rust_can::relay::BackPressurePolicy;

    let bus = Arc::new(VirtualBus::new());
    let rx = bus
        .subscribe(
            vec![],
            SubscriberOptions {
                channel_depth: 2,
                back_pressure: BackPressurePolicy::DropOldest,
                rate_limit_per_sec: 0,
            },
        )
        .await
        .unwrap();

    for i in 0u32..5 {
        bus.send(
            Context::background(),
            Frame {
                id: i & 0x7FF,
                data: vec![i as u8],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    // DropOldest keeps the newest 2 frames (ids 3 and 4).
    let f1 = rx.recv().await.unwrap();
    let f2 = rx.recv().await.unwrap();
    assert_eq!(f1.id, 3);
    assert_eq!(f2.id, 4);
}

// ---------------------------------------------------------------------------
// Security tests
// ---------------------------------------------------------------------------

//fusa:sec-test REQ-SEC-001
#[test]
fn sec_frame_id_bounds_injection_prevention() {
    use rust_can::validate_frame;
    // Standard ID injection attempt: ID > 0x7FF
    assert!(validate_frame(&Frame {
        id: 0x800,
        ..Default::default()
    })
    .is_err());
    // Extended ID injection attempt: ID > 0x1FFFFFFF
    assert!(validate_frame(&Frame {
        id: 0x2000_0000,
        ext: true,
        ..Default::default()
    })
    .is_err());
    // Boundary values must be accepted
    assert!(validate_frame(&Frame {
        id: 0x7FF,
        ..Default::default()
    })
    .is_ok());
    assert!(validate_frame(&Frame {
        id: 0x1FFF_FFFF,
        ext: true,
        ..Default::default()
    })
    .is_ok());
}

//fusa:sec-test REQ-SEC-002
#[test]
fn sec_e2e_crc_tamper_detection() {
    use rust_can::safety::{Config, E2EErrorKind, Protector, Receiver};

    let cfg = Config {
        data_id: 0xABCD,
        source_id: 0x1234,
    };
    let protector = Protector::new(cfg);
    let receiver = Receiver::new(cfg);

    let payload = b"safety-critical-command";
    let protected = protector.protect(payload);

    // Tamper with the CRC bytes directly (bytes 8-9 of the header)
    let mut tampered = protected.clone();
    tampered[8] ^= 0xFF;
    let err = receiver.unwrap(&tampered).unwrap_err();
    assert_eq!(err.kind, E2EErrorKind::CrcMismatch);

    // Tamper with the payload
    let mut tampered2 = protected.clone();
    tampered2[10] ^= 0x01;
    let receiver2 = Receiver::new(cfg);
    let err2 = receiver2.unwrap(&tampered2).unwrap_err();
    assert_eq!(err2.kind, E2EErrorKind::CrcMismatch);
}

//fusa:sec-test REQ-SEC-003
#[test]
fn sec_replay_detection_via_sequence_counter() {
    use rust_can::safety::{Config, E2EErrorKind, Protector, Receiver};

    let cfg = Config {
        data_id: 0x0001,
        source_id: 0x0001,
    };
    let protector = Protector::new(cfg);
    let receiver = Receiver::new(cfg);

    let p0 = protector.protect(b"frame 0");
    let _p1 = protector.protect(b"frame 1");
    let p2 = protector.protect(b"frame 2");

    receiver.unwrap(&p0).unwrap();
    // Replay frame 0 after frame 2 is skipped — should detect gap
    let err = receiver.unwrap(&p2).unwrap_err();
    assert_eq!(err.kind, E2EErrorKind::SequenceGap);
}

//fusa:sec-test REQ-SEC-004
#[tokio::test]
async fn sec_isotp_timeout_prevents_resource_exhaustion() {
    use rust_can::isotp::{Config, IsoTpConn};

    let bus = Arc::new(VirtualBus::new());
    let cfg = Config {
        tx_id: 0x7E0,
        rx_id: 0x7E8,
        timeout: std::time::Duration::from_millis(20),
        ..Default::default()
    };
    let conn = IsoTpConn::new(bus, cfg).await.unwrap();
    // No frames arrive — recv must return Timeout, not block forever
    let result = conn.recv(Context::background()).await;
    assert!(matches!(result, Err(rust_can::Error::Timeout)));
}

//fusa:sec-test REQ-SEC-005
#[test]
fn sec_dbc_parse_no_panic_on_malformed_input() {
    use rust_can::dbc::parse;

    let malformed_inputs = [
        "",
        "BO_ not_a_number Name: 4 ECU",
        "SG_ Signal : 0|0@1+ (0,0) [] \"\" Vector__XXX",
        &"A".repeat(65536),
        "BO_ 100 X: 0 ECU\n SG_ S : 999|999@1+ (0,0) [] \"\" V",
        "\x00\x01\x02\x03",
    ];

    for input in &malformed_inputs {
        // Must not panic — result may be Ok or Err, but never a panic
        let _ = parse(input);
    }
}

// ---------------------------------------------------------------------------
// Rate limiting (REQ-SEC-007)
// ---------------------------------------------------------------------------

//fusa:sec-test REQ-SEC-007
#[tokio::test]
async fn sec_rate_limit_drops_excess_frames() {
    let bus = Arc::new(VirtualBus::new());
    let rx = bus
        .subscribe(
            vec![],
            SubscriberOptions {
                channel_depth: 64,
                back_pressure: BackPressurePolicy::DropNewest,
                rate_limit_per_sec: 3,
            },
        )
        .await
        .unwrap();

    // Send 10 frames rapidly — only the first 3 should be accepted in the window.
    for i in 0u32..10 {
        let _ = bus
            .send(
                Context::background(),
                Frame {
                    id: i & 0x7FF,
                    data: vec![i as u8],
                    ..Default::default()
                },
            )
            .await;
    }

    // Drain what was accepted.
    let mut count = 0usize;
    while let Ok(Some(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(10), rx.recv()).await
    {
        count += 1;
    }
    assert!(
        count <= 3,
        "rate limit of 3/s must drop excess frames; got {}",
        count
    );
}

// ---------------------------------------------------------------------------
// Bus-off error exposure (REQ-SEC-008)
// ---------------------------------------------------------------------------

//fusa:sec-test REQ-SEC-008
#[test]
fn sec_bus_off_error_is_distinct() {
    // Error::BusOff must be distinct from Closed/Timeout so callers can
    // implement correct recovery policies.
    let e = rust_can::Error::BusOff;
    assert!(e.kind().is_none()); // not a RELAY sentinel
    assert_eq!(e.to_string(), "can: bus-off");
    assert!(!matches!(e, rust_can::Error::Closed));
    assert!(!matches!(e, rust_can::Error::Timeout));
}

// ---------------------------------------------------------------------------
// MessageAuthenticator trait (REQ-SEC-006)
// ---------------------------------------------------------------------------

//fusa:sec-test REQ-SEC-006
#[test]
fn sec_message_authenticator_trait_is_object_safe() {
    use rust_can::safety::MessageAuthenticator;

    // Verify the trait is object-safe by constructing a dyn reference.
    struct NullAuth;
    impl MessageAuthenticator for NullAuth {
        fn sign(&self, _key: &[u8], data: &[u8]) -> Vec<u8> {
            // NOT a real MAC — for structural test only.
            data.iter()
                .fold(0u8, |acc, &b| acc.wrapping_add(b))
                .to_le_bytes()
                .to_vec()
        }
        fn verify(&self, key: &[u8], data: &[u8], tag: &[u8]) -> bool {
            self.sign(key, data) == tag
        }
        fn tag_len(&self) -> usize {
            1
        }
    }

    let auth: &dyn MessageAuthenticator = &NullAuth;
    let key = b"test-key";
    let data = b"safety-critical-payload";
    let tag = auth.sign(key, data);
    assert_eq!(tag.len(), auth.tag_len());
    assert!(auth.verify(key, data, &tag));

    // Tampered data must not verify.
    let mut bad = data.to_vec();
    bad[0] ^= 0x01;
    assert!(!auth.verify(key, &bad, &tag));
}
