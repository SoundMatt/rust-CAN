# rust-CAN Safety Case

**Standard:** ISO 26262 Part 6 (Software)  
**ASIL Level:** ASIL-B  
**Version:** 0.1.0  
**Date:** 2026-06-18

---

## 1. Safety Goal

**SG-CAN-01:** The rust-CAN library shall not transmit or accept malformed CAN frames that could disrupt bus communication or corrupt application data.

**ASIL:** B  
**Rationale:** CAN bus arbitration errors and malformed payloads can cause cascading failures across all nodes on a shared bus.

---

## 2. Hazard Analysis (HARA)

| Hazard ID | Hazard | Severity | Exposure | Controllability | ASIL |
|---|---|---|---|---|---|
| H-CAN-01 | Malformed frame accepted and transmitted | S2 | E3 | C2 | ASIL-B |
| H-CAN-02 | Safety-critical payload corrupted without detection | S3 | E2 | C2 | ASIL-B |
| H-CAN-03 | Frame replay attack accepted as authentic | S3 | E2 | C2 | ASIL-B |
| H-CAN-04 | Subscriber channel overflow causing silent frame loss | S1 | E4 | C3 | QM |
| H-CAN-05 | Application blocked indefinitely on ISO-TP timeout | S2 | E2 | C1 | ASIL-A |

---

## 3. Safety Requirements

| ID | Requirement | Implementation | Test |
|---|---|---|---|
| REQ-CAN-004 | ValidateFrame shall reject all malformed frames | `src/frame.rs:validate_frame` | `test validate_frame_*` |
| REQ-CAN-008 | Close shall be idempotent; operations after close return Closed | `src/virtual_bus/mod.rs`, `src/mock/mod.rs` | `test close_is_idempotent` |
| REQ-CAN-009 | Standard CAN ID must not exceed 0x7FF | `src/frame.rs:validate_frame` | `test validate_frame_standard_id_boundary` |
| REQ-CAN-010 | Extended CAN ID must not exceed 0x1FFFFFFF | `src/frame.rs:validate_frame` | `test validate_frame_extended_id_boundary` |
| REQ-CAN-011 | BRS=true requires FD=true | `src/frame.rs:validate_frame` | `test validate_frame_brs_requires_fd` |
| REQ-CAN-012 | RTR must be false when FD=true | `src/frame.rs:validate_frame` | `test validate_frame_rtr_fd_exclusive` |
| REQ-CAN-013 | Data length bounded by frame format (8/64/2048) | `src/frame.rs:validate_frame` | `test validate_frame_data_length` |
| REQ-CAN-014 | XL and FD are mutually exclusive | `src/frame.rs:validate_frame` | `test validate_frame_fd_xl_mutual_exclusion` |
| REQ-SAFETY-002 | CRC-16/CCITT-FALSE protects all E2E payloads | `src/safety/mod.rs:Protector::protect` | `test safety_protect_unwrap_roundtrip` |
| REQ-SAFETY-003 | Monotonically increasing sequence counter | `src/safety/mod.rs:Protector` | `test safety_sequence_counter_increments` |
| REQ-SAFETY-004 | CRC mismatch returns E2EError::CrcMismatch | `src/safety/mod.rs:Receiver::unwrap` | `test safety_crc_mismatch_detected` |
| REQ-SAFETY-005 | Sequence gap returns E2EError::SequenceGap | `src/safety/mod.rs:Receiver::unwrap` | `test safety_sequence_gap_detected` |
| REQ-ISOTP-005 | ISO-TP context timeout bounds all blocking receives | `src/isotp/mod.rs:IsoTpConn::recv` | `test isotp_timeout_respected` |

---

## 4. Verification Summary

| Activity | Tool | Evidence |
|---|---|---|
| Unit tests | `cargo test` | CI artifact: test output |
| Static analysis | `rsfusa analyze` | `check-report.json` |
| Coding standard | `rsfusa lint` | `lint-report.json` (ISO 26262 Part 6) |
| Requirement traceability | `rsfusa trace` | `trace.json` |
| Complexity (V(G)) | `rsfusa comp` | `comp-report.json` |
| FMEA | `rsfusa fmea` | `fmea.json` |
| Threat analysis | `rsfusa tara` | `tara.json` |
| Tool qualification | `rsfusa qualify` | `qualify-report.json` |
| SBOM | `rsfusa release` | `sbom.json`, `provenance.json` |

---

## 5. Argument

```
Goal:  rust-CAN does not transmit or accept malformed CAN frames (SG-CAN-01)
  ├─ Strategy: Demonstrate by structural validation + E2E protection
  │
  ├─ Claim C1: validate_frame rejects all invalid frames (H-CAN-01)
  │     Evidence: 8 targeted unit tests; 100% statement coverage of validate_frame
  │
  ├─ Claim C2: E2E safety module detects payload corruption (H-CAN-02)
  │     Evidence: CRC-16 roundtrip tests; CRC mismatch and gap detection tests
  │
  ├─ Claim C3: E2E sequence counter prevents replay (H-CAN-03)
  │     Evidence: Sequence gap detection unit test; monotonic counter enforced
  │
  ├─ Claim C4: All blocking operations bounded by context timeout (H-CAN-05)
  │     Evidence: ISO-TP timeout test; Context::with_timeout() propagated to all awaits
  │
  └─ Claim C5: No undefined behaviour in safe Rust code
        Evidence: Rust type system + Clippy -D warnings; no unsafe blocks in library code
```

---

## 6. Residual Risks

| Risk | Rationale for Acceptance |
|---|---|
| Hardware SocketCAN failures | Platform-level concern; driver and kernel-level handling outside library scope |
| J1939 source address spoofing | Application must layer E2E protection (safety module) for authenticated links |
| Subscriber drop under sustained bus flood | Back-pressure is documented; application is responsible for consumer throughput |
