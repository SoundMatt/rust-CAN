# Incident Response Procedure

**Applicability:** rust-CAN library (ISO 26262 ASIL-B component)

## Severity Levels

| Level    | Criteria                                                        | SLA        |
|----------|-----------------------------------------------------------------|------------|
| Critical | Safety goal violation, data integrity failure, field incident   | 4 hours    |
| High     | Security vulnerability with exploit PoC, data loss             | 24 hours   |
| Medium   | Regression breaking ASIL-B requirements, significant bug       | 5 days     |
| Low      | Minor bugs, documentation errors, cosmetic defects             | Next sprint|

## Roles

| Role               | Responsibility                                              |
|--------------------|-------------------------------------------------------------|
| Incident Lead      | Coordinates response, owns the postmortem                   |
| Safety Engineer    | Assesses ASIL-B impact, updates HARA/FMEA                  |
| Security Reviewer  | Assesses TARA impact, manages CVE/advisory                 |
| Release Engineer   | Produces patched release, updates SBOM                      |

## Response Steps

### 1. Detection & Triage (≤ 1 hour)
- Log the incident in the GitHub security advisory tracker
- Assign severity level using the table above
- Assign an Incident Lead
- For Critical/High: page Safety Engineer and Security Reviewer immediately

### 2. Containment (≤ SLA)
- For Critical: draft a workaround or configuration mitigation and publish it
- For security issues: coordinate CVE assignment via GitHub advisory
- Do NOT disclose publicly until a patch is available (90-day embargo max)

### 3. Analysis
- Identify root cause in terms of:
  - Which requirement(s) failed (`REQ-XXX-NNN`)
  - Which HARA hazard was realised (`H-CAN-NN`)
  - Which TARA threat materialised (if applicable)
- Update `.fusa-hara.json`, `fmea.json`, `tara.json` as needed

### 4. Fix & Verify
- Create a branch `fix/incident-YYYY-MM-DD-<slug>`
- Write a regression test with `//fusa:req` annotation covering the failure
- Run full CI including ASIL-B safety job
- Obtain review from Safety Engineer and one additional maintainer

### 5. Release
- Version bump following semantic versioning
- Publish GitHub security advisory
- Update `CHANGELOG` with security/safety entry
- Run `rsfusa release --dir .` and `rsfusa audit-pack` to regenerate SBOM and provenance

### 6. Postmortem (within 5 days of resolution)
- Root cause analysis document in `docs/incidents/`
- Lessons learned and preventive actions
- Update safety case if safety argument changed

## Contacts

- Maintainer: Matt Jones ([@SoundMatt](https://github.com/SoundMatt))
- Security advisories: https://github.com/SoundMatt/rust-CAN/security/advisories
