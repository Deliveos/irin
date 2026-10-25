## Summary

<!-- What changes and why. Keep it short. -->
<!-- One line per issue: Fixes #N (or Part of #N). -->

## How it was tested

**Automated:**

- [ ] `cargo +nightly fmt --all`
- [ ] `cargo clippy --all --all-targets`
- [ ] `cargo test --all --exclude niri-visual-tests`

**Manual:**

<!-- What you did by hand and what you saw, or write "Not tested by hand." -->

## Notes for reviewers

<!-- Unobvious design decisions and the alternatives you considered. -->
<!-- For new config syntax, give an example. -->

## Checklist

<!-- Tick the items that apply; delete the ones that don't. -->

- [ ] One commit per issue, each with `Fixes #N` / `Part of #N`; every commit builds.
- [ ] New config options are documented in `docs/wiki/` with `<sup>Since: next release</sup>` and added to the config parsing test.
- [ ] New layout operations are added to `Op` in `src/layout/tests.rs`.

<!-- Guidelines: https://github.com/Deliveos/irin/blob/main/CONTRIBUTING.md and AGENTS.md -->
