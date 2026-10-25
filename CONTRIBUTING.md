# Contributing to this niri fork

Thanks for your interest!
This is a fork of [niri](https://github.com/niri-wm/niri).
People and AI agents are both welcome to contribute: read the code, work on issues, review and open pull requests.
Agents (and anyone new to the codebase) should start with [`AGENTS.md`](AGENTS.md), which covers orientation, how to pick work, the git workflow and the checks to run.

## AI agents and upstream

You may use AI agents to write code, tests, issues, pull request descriptions and reviews here.
You don't need to say whether you used one; a `Co-Authored-By:` trailer naming the model is welcome but not required.

The person who runs the agent (or the pull request author) is responsible for the pull request.
They must understand the change and be able to answer review on it.

These rules apply only to this fork.
Upstream [niri-wm/niri](https://github.com/niri-wm/niri) does not allow LLM-written code, issues or pull request descriptions (see its [`CONTRIBUTING.md`](https://github.com/niri-wm/niri/blob/main/CONTRIBUTING.md)).
Agents never open pull requests, issues, comments or reviews in `niri-wm/niri`.
Every pull request targets `main` of `Deliveos/niri`; since this repository is a GitHub fork, set the base repository explicitly (with `gh`: `gh repo set-default Deliveos/niri`).

## Quality bar

These apply to everyone, whether a person or an agent wrote the change.

- Keep pull request descriptions, issues and comments short and to the point.
- Solve the problem at hand. Don't add complexity for problems that don't exist or are impractical.
- Tests check outcomes, not internal state. Don't repeat yourself or test things that are trivially true.
- Follow the style and structure of the surrounding code and of similar modules (e.g. when implementing a new protocol, look at the other protocol implementations).

## Issues

- Check for duplicates and point them out.
- Check whether an issue is likely an application bug rather than a niri bug.
    - Try to reproduce on another non-Smithay-based compositor (sway, KDE/KWin, GNOME/Mutter). If the issue reproduces, it's likely an application bug.
    - Try to reproduce on another *Smithay-based* compositor ([cosmic-comp], [anvil]). If the issue reproduces only on Smithay compositors, it may be a Smithay bug.
    - Make sure you're testing the Wayland version of the app on all compositors. Apps may silently use X11 when an X11 `$DISPLAY` is available.
    - Problems with X11 apps belong to [xwayland-satellite]. When testing xwayland-satellite on different compositors, make sure you use xwayland-satellite's `$DISPLAY` (rather than another compositor's built-in Xwayland `$DISPLAY`).
    - After testing, write where you could and couldn't reproduce, as well as the exact steps to reproduce if the issue is missing them.
- If an issue is a duplicate, or not a niri issue (application bug, hardware problem, configuration problem), it should be closed.

## Testing pull requests

Build the pull request and give it a go.
`docs/wiki/Development:-Developing-niri.md` has guidance on running niri test builds.

Be thorough; we're striving for polished features, so point out any issues, even small ones like animation jank.

- Think of weird edge cases or unexpected interactions and try them.
- Try to break the feature and check that it behaves well.
- Where applicable, try different input devices: keyboard, mouse, trackpad, tablet, touchscreen.
- Watch out for performance drops.

For bug fixes, first reproduce the bug, then repeat the same steps on the pull request build and verify that the bug is fixed.
Test similar and related edge cases too, to check that the fix doesn't introduce new problems.

Write your findings in the pull request, and re-test after the author updates the code.
Only report testing you actually did.

## Reviewing pull requests

- Check the logic and edge cases; look for scenarios the author didn't handle.
- Check that the code fits into the rest of niri and follows its design and code style.
- Check for unrelated changes that should be a separate pull request.
- Check that the wiki was updated where needed (e.g. new config options documented with examples and `<sup>Since: next release</sup>`).

Point out everything you find as review comments, and be constructive and respectful.
If everything looks good, say so.
People and agents with write access may approve and merge once CI is green on the latest commit and no review threads are unresolved.
A pull request whose latest commit has no CI run is not green.

## Writing pull requests

- Make sure new features fit niri's design (`docs/wiki/Development:-Design-Principles.md`). Ideally there's an issue where the solution was agreed on.
- Link the issues you fix. One commit per issue, with `Fixes #N` (or `Part of #N`) in the commit message.
    Closely related issues may share a pull request; unrelated ones get their own.
- Keep pull requests free of unrelated changes.
- Document unobvious design and architecture decisions: why you chose one way over another, and which alternatives you tried.
- Every commit should build and pass tests. This makes review and bisecting much easier.
    - When addressing review comments, squash the changes into the relevant commits.
    - Update from `main` by rebasing, not merging.
    - [git-rebase.io](https://git-rebase.io/) is a helpful guide for splitting commits and cleaning up history.
- Mark review comments as resolved once you address them.
- Run `cargo +nightly fmt --all`, `cargo clippy --all --all-targets` and `cargo test --all --exclude niri-visual-tests` (see Tests in `docs/wiki/Development:-Developing-niri.md`). CI runs the rest, except the docs build: if you change anything under `docs/`, run `uv run mkdocs build` there (see `AGENTS.md`).
- For new layout operations, add them to the randomized tests (`Op` in `src/layout/tests.rs`). For weird Wayland handling, client-server tests in `src/tests/` are very useful. Crash fixes come with a regression test where possible.
- Document new config options in `docs/wiki/` and add them to the config parsing test.
- Fill in the pull request template, including how you tested the change.


[cosmic-comp]: https://github.com/pop-os/cosmic-comp
[anvil]: https://github.com/Smithay/smithay/tree/master/anvil
[xwayland-satellite]: https://github.com/Supreeeme/xwayland-satellite
