# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✓         |

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report security issues by email to the maintainer listed in `Cargo.toml` or via GitHub's private vulnerability reporting at:

> https://github.com/SoundMatt/rust-CAN/security/advisories/new

Include:
- A description of the vulnerability and its impact
- Steps to reproduce or a proof-of-concept
- Affected versions
- Suggested fix (if known)

You will receive acknowledgement within 5 business days and a resolution timeline within 14 days.

## Security Scope

rust-CAN processes CAN bus frames from external sources. The threat model includes:

| Threat                        | CWE          | Mitigation                              |
|-------------------------------|--------------|-----------------------------------------|
| Malformed frame injection     | CWE-20       | `validate_frame()` rejects bad frames   |
| Payload corruption (silent)   | CWE-354      | E2E CRC-16 protection (`safety` module) |
| Replay / sequence attacks     | CWE-294      | E2E sequence counter                    |
| Buffer overflow (data)        | CWE-125      | Bounded data lengths enforced at parse  |
| Channel overflow (DoS)        | CWE-400      | BackPressure policy per subscriber      |

Full threat analysis: see `tara.json`.

## ASIL-B Safety Artifacts

This library targets ISO 26262 ASIL-B. Safety artifacts are produced on every CI run:

- `fmea.json` — Failure Mode and Effects Analysis
- `tara.json` — Threat Analysis and Risk Assessment
- `safety-case.json` / `safety-case.md` — GSN safety case
- `.fusa-hara.json` — Hazard Analysis and Risk Assessment

## Incident Response

See `INCIDENT-RESPONSE.md` for the incident response procedure.
