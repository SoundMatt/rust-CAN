# rsfusa v0.2.8 — Tool Qualification Evidence (TQL-2)

**Standard reference:** ISO 26262-8:2018 §11 (Methods for software tool qualification)  
**Tool Qualification Level:** TQL-2  
**Tool version:** 0.2.8  
**Date prepared:** 2026-06-19  
**Prepared by:** Matt Jones  

---

## 1. Tool Identification

| Field | Value |
|---|---|
| Tool name | rsfusa |
| Version | 0.2.8 |
| Vendor | rsfusa project (open source) |
| Tool type | Static analysis / requirement traceability linter |
| Platform | macOS / Linux (Rust toolchain) |
| Invocation | `rsfusa trace`, `rsfusa verify`, `rsfusa annotate` |

---

## 2. Tool Usage in this Project

rsfusa is used to:

1. **Trace** `//fusa:req <REQ-ID>` annotations from production source files to requirement records in `.fusa-reqs.json`.
2. **Verify** `//fusa:test <REQ-ID>` annotations on test functions, confirming that each requirement has at least one test.
3. **Annotate** `//fusa:unsafe <justification>` markers on every `unsafe {}` block, enforcing review traceability.
4. **Detect** `//fusa:sec-test` markers for cybersecurity verification tests.

### 2.1 Tool Classification

rsfusa output is used to generate traceability evidence for ISO 26262 ASIL-B compliance. If rsfusa incorrectly reports coverage (false negative), unannotated safety-critical code could escape review. This makes rsfusa a **TCL-2** (Tool Confidence Level 2) tool.

Per ISO 26262-8 §11.4.6, TQL-2 requires:
- Tool validation (§11.4.6 method 1): Demonstrate correct tool operation on representative inputs.
- Software tool documentation (§11.4.6.3): Document the tool's intended function and limitations.

---

## 3. Known Limitations

### 3.1 Comma-ID Bug (Issue #rsfusa-42)

**Description:** rsfusa v0.2.8 silently ignores all requirement IDs after the first comma in a comma-separated annotation. For example:

```rust
//fusa:req REQ-A-001, REQ-A-002   // Only REQ-A-001 is traced; REQ-A-002 is silently dropped
```

**Impact:** False negatives in coverage reporting. A requirement annotated only in a multi-ID line would appear uncovered.

**Mitigation applied in this project:** All annotations in rust-CAN use exactly ONE requirement ID per line. This is enforced by code review and verified by the `audit_coverage.sh` script in the repository root.

```bash
# Verification: no multi-ID annotation lines exist in production code
grep -rn '//fusa:req.*,' src/ | wc -l  # must be 0
```

### 3.2 Macro Expansion

rsfusa operates on source text, not on macro-expanded output. Annotations inside `macro_rules!` expansion sites are not traced.

**Mitigation:** No requirement-critical code in rust-CAN is hidden behind macros. All annotated functions are in regular Rust source.

---

## 4. Tool Validation Evidence

### 4.1 Positive Trace Case

**Input:** `src/safety/mod.rs` — contains `//fusa:req REQ-SAFETY-001`.  
**Expected output:** REQ-SAFETY-001 is listed as "src-annotated" in `rsfusa trace` output.  
**Observed:** PASS — REQ-SAFETY-001 appears under `src/safety/mod.rs` in trace report.

### 4.2 Positive Test Coverage Case

**Input:** `src/safety/mod.rs` tests — contains `//fusa:test REQ-SAFETY-001`.  
**Expected output:** REQ-SAFETY-001 marked "test-covered" in `rsfusa verify` output.  
**Observed:** PASS.

### 4.3 Negative Case (Missing Annotation)

**Input:** Temporarily removed `//fusa:req REQ-SAFETY-001` from production code.  
**Expected output:** REQ-SAFETY-001 listed as "NOT src-annotated".  
**Observed:** PASS — tool correctly reported missing annotation.

### 4.4 Comma-ID Bug Confirmation

**Input:** `//fusa:req REQ-A-001, REQ-A-002` on one line.  
**Expected:** Both traced. **Observed:** Only REQ-A-001 traced.  
**Status:** Known defect, mitigated by project convention (one ID per line).

---

## 5. Configuration Used

```toml
# .fusa.toml (if present) or command-line
reqs_file = ".fusa-reqs.json"
src_dirs  = ["src"]
test_dirs = ["src", "tests"]
```

---

## 6. Sign-off

| Role | Name | Date |
|---|---|---|
| Prepared by | Matt Jones | 2026-06-19 |
| Reviewed by | — | — |
