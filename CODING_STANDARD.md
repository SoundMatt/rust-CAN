# rust-CAN Coding Standard

**Standard:** ISO 26262 Part 6 Annex D / MISRA-aligned Rust rules  
**ASIL level:** ASIL-B

---

## 1. Language and Edition

- Rust 2021 edition.
- Minimum supported Rust version (MSRV): 1.75 (stable toolchain).
- `#![forbid(unsafe_code)]` in `lib.rs` — all unsafe code requires explicit waiver via PR review.

## 2. Naming Conventions

| Item | Convention | Example |
|---|---|---|
| Types, traits | `UpperCamelCase` | `VirtualBus`, `FrameReceiver` |
| Functions, methods | `snake_case` | `validate_frame`, `send` |
| Constants | `SCREAMING_SNAKE_CASE` | `CAN_MAX_DATA_LEN` |
| Modules | `snake_case` | `virtual_bus`, `isotp` |
| Error variants | `UpperCamelCase` | `InvalidFrame`, `Closed` |

## 3. Error Handling

- Use `thiserror::Error` for all library error types.
- Never use `unwrap()` or `expect()` in library code — only in tests and `main.rs`.
- All error variants that correspond to a RELAY sentinel must implement `From<relay::Error>`.
- `?` is preferred over explicit `match` for error propagation.

## 4. Concurrency

- All shared state uses `Arc<tokio::sync::Mutex<T>>` or `Arc<std::sync::Mutex<T>>` (for short, non-async critical sections).
- Metrics counters use `std::sync::atomic::AtomicU64` with `Ordering::SeqCst`.
- No `std::thread::spawn` in library code — use `tokio::spawn` exclusively.

## 5. Async

- All blocking operations are `async fn`.
- No `tokio::runtime::Runtime::block_on` in library code.
- Use `tokio::time::timeout` for all operations with a `Context` deadline.
- Context deadline checking: `if ctx.done() { return Err(Error::Timeout); }` before every blocking await.

## 6. Safety Annotations

Every function covered by a safety requirement MUST carry a `//fusa:req` annotation:

```rust
//fusa:req REQ-CAN-004
pub fn validate_frame(f: &Frame) -> Result<(), Error> { ... }
```

Multiple requirements on one item: one annotation per line.  
Multi-line comment blocks are NOT used for annotations — single-line only.

## 7. Tests

- Every public function has at least one unit test in a `#[cfg(test)]` block.
- Every safety requirement has a dedicated test named `test_<req_id_lowercase>` or a descriptive name referencing the requirement.
- Integration tests live in `tests/`.
- Async tests use `#[tokio::test]`.
- No sleeping in tests — use channels or `Notify` for synchronisation.

## 8. Clippy

All code must compile with `cargo clippy --all-targets -- -D warnings` on the CI toolchain. No `#[allow(clippy::*)]` without a comment explaining the rationale.

## 9. Formatting

`cargo fmt` is enforced in CI. No manual formatting exceptions.

## 10. Dependencies

- Minimise dependencies. Every new dependency requires a justification comment in `Cargo.toml`.
- Use `[target.'cfg(target_os = "linux")'.dependencies]` for platform-specific crates.
- `Cargo.lock` is committed (binary project).

## 11. Documentation

- Public items (types, traits, functions, constants) have doc comments (`///`).
- Doc comments describe *what* the item does and any invariants or safety requirements.
- No `// TODO` or `// FIXME` in committed code; open a GitHub issue instead.

## 12. Module Layout

```
src/
  lib.rs          — re-exports, SpecVersion, module declarations
  relay.rs        — RELAY protocol types (bundled)
  error.rs        — Error enum
  frame.rs        — Frame, Filter, LoanedFrame, validate_frame
  bus.rs          — Bus trait, optional traits, FrameReceiver
  adapt.rs        — RELAY adapter
  virtual_bus/    — VirtualBus implementation
  mock/           — MockBus for testing
  socketcan/      — Linux SocketCAN (cfg-gated)
  isotp/          — ISO-TP transport
  j1939/          — J1939 PGN layer
  dbc/            — DBC parser
  safety/         — E2E safety protection
  bin/main.rs     — CLI binary
```
