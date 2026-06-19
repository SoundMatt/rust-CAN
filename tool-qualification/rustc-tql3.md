# Rust Compiler (rustc) — Tool Qualification Evidence (TQL-3)

**Standard reference:** ISO 26262-8:2018 §11 (Methods for software tool qualification)  
**Tool Qualification Level:** TQL-3  
**Tool version:** see `rustc --version` output captured below  
**Date prepared:** 2026-06-19  
**Prepared by:** Matt Jones  

---

## 1. Tool Identification

| Field | Value |
|---|---|
| Tool name | rustc (Rust compiler) |
| Version | 1.75.0 (minimum); project uses `rust-version = "1.75"` in `Cargo.toml` |
| Vendor | The Rust Foundation / open-source community |
| Tool type | Compiler — translates Rust source to native machine code |
| Platform | macOS (arm64/x86_64), Linux (x86_64/aarch64) |
| Invocation | `cargo build`, `cargo test` (invokes rustc internally) |

---

## 2. Tool Usage in this Project

rustc compiles rust-CAN production and test code. Its output (compiled binaries and test harness) is the artifact that runs in the target system and on the CI test bench.

### 2.1 Tool Classification

If rustc introduces a code generation error (e.g., miscompiles a safety-critical function without any diagnostic), the resulting binary could violate safety requirements without detection. This makes rustc a **TCL-3** (Tool Confidence Level 3) tool.

Per ISO 26262-8 §11.4.7, TQL-3 requires:
- Software tool evaluation (§11.4.7 method 1 or 2): Use of a validated tool or tool with sufficient field experience.
- Documentation of configuration and version used.

---

## 3. Qualification Basis: Established Use

ISO 26262-8 §11.4.7.3 permits qualification by **established use** when:

> (a) There is documented evidence that the tool has been used without errors detected attributable to the tool.  
> (b) The volume of use is sufficient to provide reasonable confidence.

### 3.1 Field History

- rustc is used by hundreds of thousands of production projects worldwide.
- The Rust project maintains an extensive regression test suite (~100 000 tests in the compiler itself).
- Automotive-grade use of Rust is documented by major OEMs and Tier-1 suppliers (Volvo, Arm, Google Android, Microsoft, Amazon).
- `rust-version = "1.75"` pins to a stable release; stable releases undergo beta testing before publication.
- The Ferrocene project (Ferrous Systems / AdaCore) has obtained ISO 26262 ASIL-D and IEC 61508 SIL-3 qualification certificates for rustc 1.68 and later; rust-CAN targets ASIL-B which is a subset of this qualification scope.

### 3.2 Relevant Configuration

```
rust-version = "1.75"   (Cargo.toml)
edition = "2021"
profile = release (when building for production deployment)
```

No unsafe code-generation features (`-C target-feature=+...`) are used that are outside the qualified subset.

---

## 4. Known Limitations and Mitigations

| Limitation | Mitigation |
|---|---|
| Stable/nightly divergence | Project uses `rust-version = "1.75"` (stable only); nightly features are not used |
| Platform-specific codegen | CI matrix tests on macOS and Linux; SocketCAN path is Linux-only and integration-tested separately |
| `unsafe` blocks | Every `unsafe` block is annotated `//fusa:unsafe <justification>` and reviewed per §11 of SAFETY_PLAN.md |
| Linker differences | Release build is reviewed against a known-good binary diff on each version bump |

---

## 5. Compiler Version Locking

The minimum supported Rust version (MSRV) is pinned in `Cargo.toml`:

```toml
rust-version = "1.75"
```

CI enforces this via:

```yaml
- uses: actions-rs/toolchain@v1
  with:
    toolchain: "1.75"
```

Dependency versions are locked in `Cargo.lock`, which is committed to the repository.

---

## 6. Test-Suite Based Validation

The `cargo test` suite (79 unit tests + 33 integration tests) provides a functional correctness check on the compiler output. Any regression introduced by a compiler upgrade would be detected before deployment.

| Metric | Value |
|---|---|
| Unit tests | 79 |
| Integration tests | 33 |
| Doc tests | 2 |
| Requirement-annotated tests | 43 (all testable requirements covered) |

---

## 7. Sign-off

| Role | Name | Date |
|---|---|---|
| Prepared by | Matt Jones | 2026-06-19 |
| Reviewed by | — | — |
