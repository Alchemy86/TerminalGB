<!-- Thanks for contributing! Keep this short and honest. -->

## What & why

<!-- What does this change, and why? Link any issue it closes (e.g. "Closes #12"). -->

## Type

- [ ] Bug fix
- [ ] Emulation accuracy fix
- [ ] Feature
- [ ] Docs / tooling / CI

## Checklist

- [ ] `make lint` passes (rustfmt + clippy `-D warnings`)
- [ ] `cargo test --release --lib` and `cargo test --release --test testroms` pass
      *(or I re-blessed a baseline and explained why below)*
- [ ] Updated `CHANGELOG.md` under `[Unreleased]` for user-facing changes
- [ ] Ran `make docs-links` if I moved source lines referenced by the docs
- [ ] For an **emulation** fix: captured it per the
      [standing convention](../CLAUDE.md) (symptom, which test ROM catches it, the hardware
      behaviour + citation, any new test)

## Notes

<!-- Baseline re-blessing rationale, testing done, screenshots, anything a reviewer needs. -->
