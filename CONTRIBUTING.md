# Contributing to rust-CAN

## DCO Sign-off

Every commit MUST include a Developer Certificate of Origin sign-off:

```
Signed-off-by: Your Name <your@email.com>
```

Use `git commit -s` to add the sign-off automatically.

## Branch Workflow

```bash
git checkout main && git pull
git checkout -b feat/<feature-name>
# implement + tests + requirements update
git commit -s
git push -u origin feat/<feature-name>
gh pr create --base main
```

## Quality Gates

Before opening a PR:

- [ ] `cargo build` succeeds
- [ ] `cargo test --all-targets` passes
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] New behaviour is traced to a `REQ-xxx-NNN` requirement in `requirements.json`
- [ ] Safety-relevant changes update `fmea.json` and `safety-case.md`
- [ ] DCO `Signed-off-by` present on all commits

## Requirements Traceability

Every safety-relevant function MUST carry a `//fusa:req REQ-xxx-NNN` annotation:

```rust
//fusa:req REQ-CAN-004
pub fn validate_frame(f: &Frame) -> Result<(), Error> { ... }
```

Requirement IDs are defined in `requirements.json`. Never reuse or renumber IDs.

## Commit Style

Use conventional commit prefixes: `feat:`, `fix:`, `test:`, `docs:`, `refactor:`, `chore:`.

## License

Contributions are licensed under MPL-2.0. See `LICENSE`.
