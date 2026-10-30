# How nori differs from niri

nori is a fork of [niri](https://github.com/niri-wm/niri).
This file lists what nori does differently from upstream niri: crash and freeze fixes, security fixes, other bug fixes, performance and memory work, and the changes that users and packagers notice.
Each entry links to the pull requests or commits that made the change and to the issues that describe the problem.

Pull requests that change how nori behaves compared with niri update this file in the same pull request (see [CONTRIBUTING.md](CONTRIBUTING.md#writing-pull-requests)).

- **Upstream base:** nori contains upstream niri up to [9d8f4f8](https://github.com/niri-wm/niri/commit/9d8f4f89f6a075a04ed24af5f13829d0bdba5b85), merged in [#117](https://github.com/Deliveos/nori/pull/117) on 2026-10-26.
    Later upstream commits are not in nori until the next merge (see [Upstream merges](#upstream-merges)).
- **Older work:** the first fixes were made in an earlier repository and moved here in [#1](https://github.com/Deliveos/nori/pull/1).
    Their commit messages use that repository's issue and pull request numbers ("Fixes #N", "Merge pull request #N"), which don't match the numbers here.
    This file links those changes by commit and gives each issue the number it has in this repository.

## Contents

- [What you notice coming from niri](#what-you-notice-coming-from-niri)
- [Names](#names)
- [Crash fixes](#crash-fixes)
- [Freeze and hang fixes](#freeze-and-hang-fixes)
- [Security and the lock screen](#security-and-the-lock-screen)
- [Other bug fixes](#other-bug-fixes)
- [Performance](#performance)
- [Memory](#memory)
- [Building and packaging](#building-and-packaging)
- [Tests and CI](#tests-and-ci)
- [Project rules](#project-rules)
- [Upstream merges](#upstream-merges)
- [Open problems](#open-problems)

## What you notice coming from niri

- The command is `nori`, the config is `~/.config/nori/config.kdl`, and the IPC socket path is in `$NORI_SOCKET`; see [Names](#names).
    nori doesn't read `~/.config/niri`.
- smithay is built from the [Deliveos/smithay](https://github.com/Deliveos/smithay) fork; see [Building and packaging](#building-and-packaging).
- Limits that niri doesn't have:
    - an IPC request line may be at most 8 MiB ([details](#ipc-request-limit));
    - at most 16 pastes of nori's own clipboard data (such as a copied screenshot) can be pending at once ([details](#clipboard-writer-threads));
    - a blur region with more than 64 rectangles is blurred over its bounding box ([details](#blur-region-cap));
    - configured output positions are clamped to ±16777216, and custom modes with zero timings are ignored ([details](#output-positions-and-custom-modes));
    - DisplayConfig rejects scales that are not finite and positive ([details](#displayconfig-scale)).
- Screenshots named from `screenshot-path` never overwrite an existing file: when the name is taken (two screenshots in the same second, or a `screenshot-path` without time fields), nori adds ` (2)` … ` (999)`, and saving fails after that; with `screenshot-path null` D-Bus screenshots go to `$XDG_RUNTIME_DIR/screenshot.png` instead of `/tmp/screenshot.png` ([details](#screenshots)).
- Only the owner of the `org.gnome.Orca.KeyboardMonitor` bus name may listen to or grab keys through the accessibility D-Bus interface ([details](#keyboardmonitor)).
- PipeWire screencasts offer 4 buffers by default instead of 8 ([details](#pipewire-buffers)).
- Starting the session with `--session` on systemd stops an `xdg-desktop-portal.service` that is already running, so it restarts with nori's environment ([details](#portal-at-session-start)).

## Names

nori renames the names that users and other programs see ([#78](https://github.com/Deliveos/nori/pull/78); fixes [#76](https://github.com/Deliveos/nori/issues/76), [#77](https://github.com/Deliveos/nori/issues/77)).

- Commands: the binary is `nori` (`nori msg`, `nori validate`, and shell completions for `nori`). No `niri` binary is built.
- Config: `$XDG_CONFIG_HOME/nori/config.kdl` (usually `~/.config/nori/config.kdl`) and `/etc/nori/config.kdl`.
    There is no fallback to the niri paths: with neither file present, nori writes a default config to the user path.
- Environment: `NORI_SOCKET`, `NORI_CONFIG` and `NORI_DISABLE_SYSTEM_MANAGER_NOTIFY`; for builds, `NORI_BUILD_VERSION_STRING` and `NORI_BUILD_COMMIT`.
    Tools that read `NIRI_SOCKET`, including ones built on upstream's `niri-ipc` crate, don't find the socket until they are updated.
- The IPC socket is `nori.<wayland display>.<pid>.sock`, `XDG_CURRENT_DESKTOP` is `nori` (so xdg-desktop-portal reads `nori-portals.conf`), and spawned programs run in `app-nori-*` systemd scopes.
- Session files: `nori-session`, `nori.service`, `nori-shutdown.target`, `nori.desktop` (shown as "Nori"), and the dinit services `nori` and `nori.target`.
- Packaging: `nori.spec.rpkg`, the Nix flake packages `nori` and `nori-debug`, the Cargo RPM and deb metadata, and the release asset `nori-<version>-vendored-dependencies.tar.xz`.
- Internal names stay niri to keep upstream merges simple: the crates (`niri`, `niri-config`, `niri-ipc`, `niri-visual-tests`), source paths and types, the `niri` log target of library messages (startup messages of the binary use `nori`), the `niri_*` uniforms of custom shaders, and the pages in `docs/wiki/`.
    The dynamic cast target also still reports the app ID `rs.bxt.niri.desktop` to the portal's window picker; only its title says nori.
    [README.md](README.md) explains how to read the upstream documentation.

## Crash fixes

A crash of the compositor ends the whole session.

- <a id="output-positions-and-custom-modes"></a>**Huge output positions and zero-size custom modes.**
    In niri, a very large output position (from the config, IPC or wlr-output-management) overflows `i32` when the next output is placed, and a custom mode such as width 0 gives zero timings that smithay divides by.
    nori clamps configured positions to ±2^24 (16777216), skips custom modes whose clock, htotal or vtotal is zero (with the warning "ignoring invalid custom mode"), and treats refresh intervals that the frame clock can't use as unknown ([71f4f158](https://github.com/Deliveos/nori/commit/71f4f158ee8a8ec87d52ee7ca9f72e2ce9b8a9f3); fixes [#15](https://github.com/Deliveos/nori/issues/15)).
- **Workspaces on removed or unfocused monitors.**
    In niri, an ext-workspace assign to an output that was removed meanwhile panics, and naming a workspace on an unfocused monitor (by id or name over IPC) checks the focused monitor instead of the workspace's own, so the workspace's monitor doesn't get the empty workspace it needs, and a later cleanup can trip an assert.
    nori ignores assigns to outputs that no longer exist and uses the monitor that holds the workspace ([ce714900](https://github.com/Deliveos/nori/commit/ce71490003636727ec1ec79a0dfa086caba67c65); fixes [#16](https://github.com/Deliveos/nori/issues/16)).
- **Valid spring animation configs.**
    In niri, a bouncy workspace-switch spring (for example `damping-ratio=0.2`) can overshoot so that no workspace is on screen, and an interactive window move then panics; an overdamped spring with a fast initial velocity makes the spring duration negative and panics.
    nori doesn't cull in that window-move code and falls back to an estimate when the computed duration is invalid ([49b38eec](https://github.com/Deliveos/nori/commit/49b38eec3f0ec501a1b7ba23e48aad2e3db32df0); fixes [#11](https://github.com/Deliveos/nori/issues/11)).
- **Left click in the window switcher with the pointer on no output** (after an unplug, an output turned off, or a scale change) panics in niri.
    nori treats it as a click outside the switcher and closes it ([b27a9740](https://github.com/Deliveos/nori/commit/b27a9740a63aaf9b8e1078be4cafe9a6ab76e89f); fixes [#10](https://github.com/Deliveos/nori/issues/10)).
- <a id="clipboard-writer-threads"></a>**Clipboard writer threads.**
    When niri itself owns the clipboard (a screenshot copied to the clipboard), it starts one thread per paste request, and each thread blocks until the client reads.
    A client that never reads piles up threads (200 requests left 200 threads in the issue's test), and when starting a thread fails, niri panics.
    nori allows at most 16 such transfers at once, drops further requests with the warning "too many pending selection writes, dropping request", and logs a failed thread start instead of panicking.
    The limit is global, so while one client holds 16 unread transfers, pastes of nori's own clipboard data fail for all clients (the request is dropped and the client reads nothing) until that client reads or closes them ([44a1124e](https://github.com/Deliveos/nori/commit/44a1124e8b3ed61087ffe4ffc20e99537236126f); fixes [#9](https://github.com/Deliveos/nori/issues/9)).
- **`stop-cast` without the ScreenCast D-Bus service.**
    `msg action stop-cast --session-id N` crashes a niri instance that runs without the ScreenCast D-Bus interface (for example one started without `--session`), even when no cast matches.
    nori returns early in that case ([e09676aa](https://github.com/Deliveos/nori/commit/e09676aa4273f2163a49fe81101551c01031bad6); fixes [#13](https://github.com/Deliveos/nori/issues/13)).
- **Empty hotkey overlay.**
    With `hotkey-overlay { hide-not-bound }` and none of the listed actions bound, niri crashes at startup, because the overlay is shown at startup by default.
    nori draws nothing in that case ([0d82a4c5](https://github.com/Deliveos/nori/commit/0d82a4c560a8a4db81193165309f3ad21d558283); fixes [#12](https://github.com/Deliveos/nori/issues/12)).
- <a id="ipc-request-limit"></a>**IPC request size limit.**
    niri reads an IPC request until a newline with no limit, so a client that never sends one (for example a script piping a large file into the socket) makes it buffer until the out-of-memory killer acts.
    nori reads at most 8 MiB (8388608 bytes) per request line.
    A longer request gets the reply `{"Err":"request is longer than 8388608 bytes"}`, the connection is closed, and the warning is logged.
    The limit was 1 MiB at first and was raised to 8 MiB because spawn requests with a long argument list (about 22000 paths) went over 1 MiB.
    `nori msg` shows that reply instead of "Broken pipe (os error 32)" since [#93](https://github.com/Deliveos/nori/pull/93), which reads the reply even when the rest of its write fails ([#64](https://github.com/Deliveos/nori/pull/64), [#73](https://github.com/Deliveos/nori/pull/73), [#93](https://github.com/Deliveos/nori/pull/93); fixes [#49](https://github.com/Deliveos/nori/issues/49), [#69](https://github.com/Deliveos/nori/issues/69), [#74](https://github.com/Deliveos/nori/issues/74)).
- <a id="displayconfig-scale"></a>**NaN scale through DisplayConfig.**
    In niri, `org.gnome.Mutter.DisplayConfig.ApplyMonitorsConfig` puts the requested scale into the output config unchecked; a NaN scale ends in a division by zero, so any process on the session bus can crash the compositor.
    nori rejects scales that are NaN, infinite, zero or negative with the D-Bus error "Invalid scale …" before applying anything (niri clamps the non-NaN ones to the 0.1–10 range) ([#96](https://github.com/Deliveos/nori/pull/96); fixes [#95](https://github.com/Deliveos/nori/issues/95)).
- **Huge `xdg_positioner` values.**
    The smithay version that niri uses does plain `i32` arithmetic on the positioner values a client sends, and release builds have overflow checks on, so one popup request such as `set_offset(i32::MAX, 0)` takes the session down.
    nori builds against a smithay fork in which this math saturates (see [Building and packaging](#building-and-packaging)) and has a regression test for it ([#101](https://github.com/Deliveos/nori/pull/101); fixes [#90](https://github.com/Deliveos/nori/issues/90)).
- **Negative `xdg_positioner` parent size.**
    In the same smithay version, a negative `set_parent_size` builds a size that fails a debug assertion, so any client can panic builds with debug assertions (the dev profile and `cargo test`); release builds were not affected.
    The smithay fork clamps negative parent sizes to 0 ([#122](https://github.com/Deliveos/nori/pull/122); fixes [#104](https://github.com/Deliveos/nori/issues/104)).

Most crash fixes from the older work (output positions, workspaces, the window switcher click, clipboard writer threads) come with client-server regression tests in `src/tests/crash_regressions.rs`, which niri doesn't have ([4a000d50](https://github.com/Deliveos/nori/commit/4a000d504c9ab0fd8b7c731a004a90a68b353dd7)); the spring fix has unit tests, the hotkey overlay fix has only a unit test of its precondition (it doesn't call `render()`), and the `stop-cast` and custom mode fixes have none.
Later crash fixes added their own cases to that file, except the DisplayConfig scale fix, whose test is a unit test in `src/dbus/mutter_display_config.rs`.

## Freeze and hang fixes

The gamma and blur fixes follow rule 2 in [AGENTS.md](AGENTS.md#3-golden-rules): don't block the main thread.
The spawn fix is about a background thread that never finished.

- **Spawning a program whose exec fails.**
    With the systemd feature (on by default), niri's spawner thread hangs forever when the exec of a spawned program fails, for example because the program is missing: each failed spawn leaves a thread and a process behind, and the error is never logged.
    In nori the intermediate child waits on its pipe with a 2-second timeout, so the spawn returns the error after about 2 seconds and nori logs it ([9029b62d](https://github.com/Deliveos/nori/commit/9029b62d31c2d32304e70ece335ddf03a9f7fb3d); fixes [#17](https://github.com/Deliveos/nori/issues/17)).
    A worry that this timeout could leave spawned programs in nori's own cgroup ([#80](https://github.com/Deliveos/nori/issues/80)) was checked and closed as not planned.
- **Gamma ramps from a pipe.**
    For wlr-gamma-control `set_gamma`, niri reads the ramps from the client's file descriptor on the main thread, so a client that passes a pipe and never writes to it freezes the compositor, including input and VT switching.
    nori reads with `pread`, which fails on pipes instead of blocking, so that request fails and nori logs "failed to read gamma data" ([f40ced8b](https://github.com/Deliveos/nori/commit/f40ced8b4e2cb176042d94867049b7d25239867a); fixes [#8](https://github.com/Deliveos/nori/issues/8)).
- **Stopping a screencast.**
    When a cast ends from the compositor's side (its window or output goes away, PipeWire fails, or the user stops it), niri runs the D-Bus `Session.Stop` code with `block_on` on the main thread, which waits for the session's interface lock (held by a running `RecordMonitor` or `RecordWindow` call on that session) and for messages to the bus, and panics if sending the `Closed` signal or removing the D-Bus objects fails.
    nori runs it on a thread and logs these errors instead ([#221](https://github.com/Deliveos/nori/pull/221); fixes [#161](https://github.com/Deliveos/nori/issues/161)).
- <a id="blur-region-cap"></a>**Huge blur regions.**
    niri converts an ext-background-effect blur region into non-overlapping rectangles on the render path, with no limit on the number of rectangles, and any client can use that protocol.
    In the issue's measurement, 4000 rectangles (about 100 KB of requests) took 9.8 seconds and produced 16 million rectangles.
    For regions with more than 64 rectangles (added and subtracted together), nori blurs the bounding box of the added rectangles instead, so holes in such a region are blurred too ([9a986ef6](https://github.com/Deliveos/nori/commit/9a986ef6e37cf5c325bd262edb64c68e82a38ea1); fixes [#7](https://github.com/Deliveos/nori/issues/7)).

## Security and the lock screen

- **Exit dialog at the lock screen.**
    In niri, an exit confirmation dialog that is open when the session locks keeps keyboard focus, so if the next key pressed at the lock screen is Enter, niri quits and the session ends.
    nori closes the dialog when locking starts ([85ecb150](https://github.com/Deliveos/nori/commit/85ecb150d83fa2a105656ef287c55d575ebb550e); fixes [#5](https://github.com/Deliveos/nori/issues/5)).
- **Locking while nori's VT is inactive.**
    In niri, a lock request made while its VT is not active (for example a before-sleep locker while the user is on another VT) fails, because nothing is rendered, and on return the desktop is shown unlocked.
    nori keeps the session in the locking state and finishes locking after switching back to its VT ([ae2d2351](https://github.com/Deliveos/nori/commit/ae2d235147b37fb5fc704f343dcdc41a5a1e72ea); fixes [#6](https://github.com/Deliveos/nori/issues/6)).
- **Window screencasts while locked.**
    niri keeps rendering window screencasts while the session is locked, so they show the window behind the lock screen; monitor casts show the lock screen.
    nori doesn't render window casts while the session is locked: not on output redraws ([5b90ed14](https://github.com/Deliveos/nori/commit/5b90ed146fd473ee84c5eb1c36af9412150cbc6c); fixes [#19](https://github.com/Deliveos/nori/issues/19)), and not on redraws that PipeWire asks for or on retargeting a dynamic cast to a window ([#137](https://github.com/Deliveos/nori/pull/137); fixes [#134](https://github.com/Deliveos/nori/issues/134)).
- <a id="keyboardmonitor"></a>**KeyboardMonitor D-Bus interface.**
    niri serves `WatchKeyboard`, `GrabKeyboard` and `SetKeyGrabs` of `org.freedesktop.a11y.KeyboardMonitor` to any caller on the session bus, so any unsandboxed program can receive every keystroke, including lock screen passwords.
    nori serves them only to the owner of the bus name `org.gnome.Orca.KeyboardMonitor` and returns `AccessDenied` to everyone else; Orca keeps working.
    This stops programs that don't own the name, but a program can still claim the name while Orca isn't running, so [Security-Model.md](docs/wiki/Security-Model.md) still applies ([84183bb6](https://github.com/Deliveos/nori/commit/84183bb6c901603c7320b4a85740f8cc53faf8bb); fixes [#20](https://github.com/Deliveos/nori/issues/20)).
- **Fixed-name D-Bus screenshot in `/tmp`.**
    With `screenshot-path null`, niri writes D-Bus screenshots to `/tmp/screenshot.png`, a fixed name in a directory that all users share.
    nori writes them to `$XDG_RUNTIME_DIR/screenshot.png` and uses the temporary directory only if no runtime directory is set ([dc91dc78](https://github.com/Deliveos/nori/commit/dc91dc785f9c440cbb9f0365a7b38972d0ffcd28); fixes [#14](https://github.com/Deliveos/nori/issues/14)).

## Other bug fixes

### Outputs

- **wlr-output-management keeps config for other outputs.**
    niri replaces the whole outputs config with the heads in a wlr-output-management request, which only covers connected outputs, so settings for unplugged monitors are lost until the output sections of the config file change (a reload with unchanged output sections keeps the applied config).
    nori replaces only the entries with the same name, in place, and keeps the order of the other entries, so a change from wlr-randr, kanshi and similar tools also takes effect when several config entries match one output ([32a109dc](https://github.com/Deliveos/nori/commit/32a109dc0c3a02c5e4620a2f8b98f9cde3f8b187), [#86](https://github.com/Deliveos/nori/pull/86); fixes [#21](https://github.com/Deliveos/nori/issues/21), [#81](https://github.com/Deliveos/nori/issues/81)).
- **DisplayConfig keeps the output entry.**
    In niri, `ApplyMonitorsConfig` (used by GNOME-style display settings) replaces the matched output entry with a new one named after the connector and with default values, so the entry loses settings such as VRR, focus-at-startup and max-bpc, and is renamed, until the output sections of the config file change.
    nori changes only the fields that DisplayConfig sets (on/off, scale, transform, position, mode; a new mode replaces a modeline) ([#94](https://github.com/Deliveos/nori/pull/94); fixes [#89](https://github.com/Deliveos/nori/issues/89)).
- **Custom modes survive an apply.**
    Clients list the current mode and send it back when they change only the scale or position; niri then stores it as a normal mode and drops a custom mode or modeline, switching the output to its preferred mode.
    nori keeps the configured mode or modeline when the requested mode is the current one, for both DisplayConfig and wlr-output-management ([#123](https://github.com/Deliveos/nori/pull/123); fixes [#99](https://github.com/Deliveos/nori/issues/99)).
- **Panel orientation applied twice.**
    niri reports the transform with the DRM panel orientation applied, and stores the transform a client sends back as the configured one, so the orientation is added a second time and the screen turns.
    nori stores the requested transform minus the panel orientation ([#123](https://github.com/Deliveos/nori/pull/123); fixes [#100](https://github.com/Deliveos/nori/issues/100)).
- **Current mode after the mode list changes.**
    Switching between a normal mode and a custom mode or modeline adds or removes a mode, and niri then doesn't tell wlr-output-management clients the new current mode, so a client that was already running shows another mode as current, and sending it back switches to that mode; turning on an output with a custom mode isn't reported to them at all.
    nori sends the current mode, and that the output is on, whenever they change ([#139](https://github.com/Deliveos/nori/pull/139); fixes [#131](https://github.com/Deliveos/nori/issues/131)).

### Screenshots

- <a id="screenshots"></a>**D-Bus screenshots reply on failure.**
    In niri, an `org.gnome.Shell.Screenshot` call whose save fails in the background thread never gets a reply, and since all calls share one reply channel, the replies to later calls go to the wrong calls; niri also doesn't create the screenshot directory on this path.
    nori gives each call its own reply, returns a D-Bus error on failure, and creates the `screenshot-path` directory ([dc91dc78](https://github.com/Deliveos/nori/commit/dc91dc785f9c440cbb9f0365a7b38972d0ffcd28); fixes [#14](https://github.com/Deliveos/nori/issues/14)).
- **No overwrites within the same second.**
    niri's default screenshot names have one-second precision, so two screenshots in the same second overwrite each other.
    For names generated from `screenshot-path`, nori adds a numbered suffix such as ` (2)` when the name is taken, up to ` (999)`; an explicit `path` given to a screenshot action is still overwritten ([dc91dc78](https://github.com/Deliveos/nori/commit/dc91dc785f9c440cbb9f0365a7b38972d0ffcd28); fixes [#14](https://github.com/Deliveos/nori/issues/14)).
- **Atomic replace of the fixed-name screenshot.**
    With `screenshot-path null`, every D-Bus screenshot goes to one file, and niri truncates and rewrites it while an earlier caller may still be reading it.
    nori writes to a temporary file next to it and renames it into place when complete ([#85](https://github.com/Deliveos/nori/pull/85); fixes [#83](https://github.com/Deliveos/nori/issues/83)).
- **Errors at the end of the PNG write.**
    niri ignores errors from the last part of a D-Bus screenshot's PNG write, so on a full disk a truncated PNG is reported as saved.
    nori reports the error, and the call fails ([#88](https://github.com/Deliveos/nori/pull/88); fixes [#84](https://github.com/Deliveos/nori/issues/84)).

### Windows and popups

- **Per-workspace `default-column-display`.**
    In niri, a new window gets its column display mode from the monitor's first workspace, so `default-column-display` in a named workspace's `layout {}` block works only when that workspace is the monitor's first one, and then applies to new windows on all of the monitor's workspaces.
    nori applies the target workspace's options before creating the column ([a31350cd](https://github.com/Deliveos/nori/commit/a31350cd6c77d3f3789a723a99f210684e4bcb6f); fixes [#22](https://github.com/Deliveos/nori/issues/22)).
- **Popups above or left of their window.**
    niri computes the output overlap of popups from the window's top-left corner, so a popup entirely above or left of its window (a tooltip above it, a menu opening upward) never gets `wl_surface.enter` and can be drawn at the wrong scale.
    nori uses an overlap area centered on the window ([#92](https://github.com/Deliveos/nori/pull/92); fixes [#72](https://github.com/Deliveos/nori/issues/72)).
- **Input method popups that move to another window.**
    When focus moves, niri doesn't send `wl_surface.leave` for the old window's output to the input method popup, so a client can keep drawing it at the old output's scale.
    nori sends leave for every output when the popup is dismissed; the popup then enters the new window's output as before ([#126](https://github.com/Deliveos/nori/pull/126); fixes [#98](https://github.com/Deliveos/nori/issues/98)).
- **Popups of layer-shell surfaces.**
    niri sends `wl_surface.enter` to a popup of a layer-shell surface only when the layer surfaces of that output are arranged (when one of them commits, maps or unmaps, or the output changes), so a new popup (a bar's menu, an input method popup in a launcher) has no output until then; since the change above, an input method popup on a layer surface also lost its output each time text input was enabled again.
    nori enters the popup on the layer surface's output right away ([#142](https://github.com/Deliveos/nori/pull/142); fixes [#132](https://github.com/Deliveos/nori/issues/132)).
- **Resize animation flash.**
    niri takes the starting snapshot of a resize animation on the commit that acknowledges the resize, and Firefox has already committed its content at the new size by then, so maximizing or restoring Firefox flashes.
    nori takes the snapshot of a window on screen when the resize is sent to it; off-screen windows, or a scale change before the window answers, fall back to niri's behavior, and with resize animations off nori skips this early snapshot (the snapshot on commit is still taken, as in niri) ([#111](https://github.com/Deliveos/nori/pull/111); fixes [#110](https://github.com/Deliveos/nori/issues/110)).
- **Cursor shape after resize and view-drag grabs.**
    In niri, the resize grab resets the cursor shape without queueing a redraw, and the spatial movement grab redraws only the output the gesture ran on, so an output that nothing else redraws (for example with the cursor over empty space on another output) keeps the resize or drag shape until its next redraw.
    nori redraws the outputs that show the cursor when the grab ends ([#65](https://github.com/Deliveos/nori/pull/65); fixes [#45](https://github.com/Deliveos/nori/issues/45)).

### Screencasts

- **Frames skipped for lack of a PipeWire buffer.**
    When a consumer holds every buffer, niri drops the frame but counts its changes as sent, so if the screen then goes still, viewers keep an old picture; it also logs a warning for every skipped frame.
    nori counts a skipped frame as not sent, so the next frame is complete, retries it (first after one frame interval, at least 10 ms, then backing off up to 1 second), and logs the warning at most once every 10 seconds per cast with the number of skipped frames ([9c5fbfb0](https://github.com/Deliveos/nori/commit/9c5fbfb0b637b45bd45c92e113e7d2833d2e26c9), [7a49ebfc](https://github.com/Deliveos/nori/commit/7a49ebfc659f599948cd0a124a1d6d092d36bb49), [#68](https://github.com/Deliveos/nori/pull/68); fixes [#34](https://github.com/Deliveos/nori/issues/34)).
- **Frame pacing.**
    niri moves frames sent slightly early forward to the ideal time, so the time base ends up ahead of the frame that was sent; a second redraw for the same target then sends a duplicate frame and logs "target frame time is below last".
    A frame late by almost a whole interval could also let a duplicate through, for example on a 60 fps cast of a 59.94 Hz output.
    nori moves only late frames back to the ideal time, and restarts the time base after a frame that is almost an interval late ([#115](https://github.com/Deliveos/nori/pull/115), [#120](https://github.com/Deliveos/nori/pull/120); fixes [#114](https://github.com/Deliveos/nori/issues/114), [#118](https://github.com/Deliveos/nori/issues/118)).
- **Render errors.**
    After a cast frame fails to render, its buffer can't be bound, or its pixels can't be copied into a shared-memory buffer later, nori sends the next frame in full and, with the cursor sent as metadata, sends a changed cursor bitmap again.
    Mostly this fixes problems that the buffer-age and SHM capture changes below brought in; resetting the cursor state and the reset after a failed bind are new compared with niri ([#120](https://github.com/Deliveos/nori/pull/120), [#138](https://github.com/Deliveos/nori/pull/138); fixes [#116](https://github.com/Deliveos/nori/issues/116), [#129](https://github.com/Deliveos/nori/issues/129)).
    When a cursor bitmap fails to render, niri sends a transparent one and nothing more until the bitmap changes, so the consumer shows no cursor; nori sends the bitmap again with the next frame ([#138](https://github.com/Deliveos/nori/pull/138); fixes [#130](https://github.com/Deliveos/nori/issues/130)).

### Session

- <a id="portal-at-session-start"></a>**Portal started before the session.**
    xdg-desktop-portal picks its backends once at startup from `XDG_CURRENT_DESKTOP`.
    If it was restarted between two sessions (after the variable was unset at logout), it uses the GTK backend, which has no screencasting, for the whole next session.
    When started with `--session` on systemd, nori stops a running `xdg-desktop-portal.service` without waiting, and D-Bus activation starts it again with nori's environment ([#113](https://github.com/Deliveos/nori/pull/113); fixes [#112](https://github.com/Deliveos/nori/issues/112)).
- **`disable-power-key-handling` on a live reload.**
    niri takes the logind power-key inhibitor only at startup, but its own power-key action follows the live config, so removing the option on a reload makes niri suspend while logind also acts on the key, by default powering off.
    In session instances, nori takes or drops the inhibitor when this option changes, and takes it on a separate thread, since the D-Bus call blocks ([9004e44c](https://github.com/Deliveos/nori/commit/9004e44cb5860979055984c29b05964ef45574ec), [#141](https://github.com/Deliveos/nori/pull/141); fixes [#23](https://github.com/Deliveos/nori/issues/23), [#135](https://github.com/Deliveos/nori/issues/135)).

## Performance

Numbers in this section are the issues' measurements or estimates; most come from a benchmark that is kept outside the repository ([#58](https://github.com/Deliveos/nori/issues/58)).

### Rendering and redraws

- **Off-screen columns.**
    niri builds render elements for every column of the workspace.
    nori skips columns that are more than half a screen outside the visible area, taking animations, focus rings, shadows and the overview zoom into account ([#3](https://github.com/Deliveos/nori/pull/3); fixes [#26](https://github.com/Deliveos/nori/issues/26)).
- **Pointer motion and cursor changes.**
    niri redraws every output on each pointer motion and each cursor change.
    nori works out which outputs the cursor and the drag-and-drop icon cover and redraws only the outputs where they appeared, moved or disappeared.
    Motion still redraws every output while the screenshot UI, the window switcher, drag-and-drop or a pointer grab other than a plain click is active, and on a focus-follows-mouse focus change or a hot-corner overview toggle; tablet tool motion, tablet proximity, tablet cursor image changes, pointer-constraint warps and warps by nori itself (warp-mouse-to-focus, moving the cursor to another monitor) still redraw every output, as in niri ([f0c5a85d](https://github.com/Deliveos/nori/commit/f0c5a85d924113c993dd703112a63a11a0242416), [a8072ea2](https://github.com/Deliveos/nori/commit/a8072ea22415e7c3fcb62a7e65ad955a5f238bb1), [d4a4a362](https://github.com/Deliveos/nori/commit/d4a4a3628e54c64ced32912b095036c3df481d62); fixes [#27](https://github.com/Deliveos/nori/issues/27)).
- **Animated cursors.**
    While the cursor is animated (for example a busy cursor), niri keeps every output redrawing at its refresh rate, about 430 redraws per second on three 144 Hz outputs according to the issue.
    nori keeps redrawing only the outputs that show the cursor ([d2a94151](https://github.com/Deliveos/nori/commit/d2a9415143000b03ef96c187e21deab9cb376b88), [c9750ca1](https://github.com/Deliveos/nori/commit/c9750ca1da156022b46826b155e007c5884c556b); fixes [#28](https://github.com/Deliveos/nori/issues/28)).
- **Failed on-demand VRR.**
    When enabling on-demand VRR fails, niri tries again on every frame, logging a warning and refreshing the output list each time.
    nori tries again only when the on-demand request turns off and on again, or when the output configuration is applied again, for example by a config reload that changes the output settings ([9fa12722](https://github.com/Deliveos/nori/commit/9fa1272271853364761d7ba068c514cf9fc63e2f); fixes [#31](https://github.com/Deliveos/nori/issues/31)).
- **Exit dialog texture.**
    niri uploads the exit confirmation dialog's texture again on every frame it is shown.
    nori uploads it once per scale while the dialog is shown ([9ecf467e](https://github.com/Deliveos/nori/commit/9ecf467ead4c952adfe70d1df72085b028e41f88); fixes [#43](https://github.com/Deliveos/nori/issues/43)).
- **Shadows turned off.**
    niri computes shadow geometry for every tile on every frame even when shadows are off (the default).
    nori skips it when shadows are off ([0d07ecd1](https://github.com/Deliveos/nori/commit/0d07ecd11d434564bb3c58e98be90879fc46cddd); fixes [#44](https://github.com/Deliveos/nori/issues/44)).

### Work on every event loop iteration

niri does some bookkeeping on every iteration of its event loop, even when nothing changed.

- **Tiled state.**
    niri recomputes every window's tiled state on every iteration.
    nori updates it when its inputs change: decoration requests, window rule changes, mapping, and config reloads ([80fa2573](https://github.com/Deliveos/nori/commit/80fa25737cd83da302e2664bef83bbb6abcc30c0), [f364ab12](https://github.com/Deliveos/nori/commit/f364ab12da5bb2f8645d7a983bcf049025656a45); fixes [#41](https://github.com/Deliveos/nori/issues/41)).
- **Pointer inactivity timer.**
    With `cursor { hide-after-inactive-ms }`, niri removes and recreates a timer on every iteration with pointer activity.
    nori keeps one timer that reschedules itself ([3b5388f2](https://github.com/Deliveos/nori/commit/3b5388f2fb71029fba5b72650f3ea7de0501cea2); fixes [#40](https://github.com/Deliveos/nori/issues/40)).
- **Screencast output tracking.**
    niri's `refresh_mapped_cast_outputs` builds IPC window layouts it doesn't use and a new set of all windows on every iteration, even with no screencast running.
    nori skips the layouts and marks windows with a generation counter instead ([a37382a1](https://github.com/Deliveos/nori/commit/a37382a1b0dcf78153f55b557899e0f5afe214da), [3c601ac7](https://github.com/Deliveos/nori/commit/3c601ac7a2ced1d07186b6bd1225cc92cab69e69); fixes [#38](https://github.com/Deliveos/nori/issues/38)).
- **Foreign toplevel close detection.**
    niri's foreign-toplevel refresh looks up every tracked toplevel with a linear search, which is O(n²) in the number of windows (8 µs per iteration at 100 windows and 24.5 µs at 200 in the issue's measurement), and builds unused IPC layouts.
    nori marks the windows it sees in one pass, and looks windows up without cloning their surface handle ([81f7c1f3](https://github.com/Deliveos/nori/commit/81f7c1f3829193f40f697bcdaf3207956583cb2a), [2d42ddb0](https://github.com/Deliveos/nori/commit/2d42ddb0a82f8823a68a309f686143187d561ecb), [#66](https://github.com/Deliveos/nori/pull/66); fixes [#37](https://github.com/Deliveos/nori/issues/37), [#54](https://github.com/Deliveos/nori/issues/54)).
    As a side effect, `closed` events now come after the updates of other windows and before the newly focused window's update.
- **IPC window list.**
    niri asks each window for its size four times while refreshing the IPC window list, and builds a new set of all window ids to find closed windows.
    nori asks once, and collects window ids only when a window was closed ([#59](https://github.com/Deliveos/nori/pull/59); fixes [#50](https://github.com/Deliveos/nori/issues/50), [#55](https://github.com/Deliveos/nori/issues/55)).
- **Dead output surfaces.**
    niri prunes dead surfaces from its outputs on every iteration; nori does it on its existing one-second timer ([#62](https://github.com/Deliveos/nori/pull/62); fixes [#56](https://github.com/Deliveos/nori/issues/56)).
- **Configure state.**
    niri clones and compares every window's configure state on every iteration.
    nori skips setters whose value didn't change and drops a pending state that equals the last one sent ([#70](https://github.com/Deliveos/nori/pull/70); fixes [#52](https://github.com/Deliveos/nori/issues/52)).
- **Window outputs.**
    niri walks every window's surface tree on every iteration to send `wl_surface.enter`.
    nori does it only for windows whose surface tree or popups committed ([#71](https://github.com/Deliveos/nori/pull/71); fixes [#51](https://github.com/Deliveos/nori/issues/51)).
    Input method popups moved to another window without a commit missed `wl_surface.enter` because of this; [#92](https://github.com/Deliveos/nori/pull/92) fixed that ([#75](https://github.com/Deliveos/nori/issues/75)).

### Screencasts and screen capture

- **GetWindows D-Bus call.**
    On each call, niri's `GetWindows` D-Bus handler, used by the screencast window picker, builds IPC layouts for all windows and throws them away; nori doesn't build them ([#63](https://github.com/Deliveos/nori/pull/63); fixes [#57](https://github.com/Deliveos/nori/issues/57)).
- **Cursor-only changes.**
    With the cursor sent as PipeWire metadata, niri renders the whole frame again when only the cursor moved.
    nori sends a buffer that carries only the cursor metadata; this was tested with OBS ([#107](https://github.com/Deliveos/nori/pull/107); fixes [#32](https://github.com/Deliveos/nori/issues/32)).
- **Buffer age.**
    niri renders every DMA-BUF cast frame in full (buffer age 0), even when a few pixels changed.
    nori passes the real buffer age for PipeWire DMA-BUF casts, so only the damaged area is rendered; DMA-BUF frames for screencopy and image-copy-capture clients are still rendered in full.
    It helps only while at most 5 buffers rotate, which the new default of 4 buffers allows ([#108](https://github.com/Deliveos/nori/pull/108); fixes [#29](https://github.com/Deliveos/nori/issues/29)).
- **SHM captures.**
    For screencopy, image-copy-capture and PipeWire casts without DMA-BUF, niri allocates a full-size texture for every frame and waits on the main thread for the GPU readback.
    nori reuses one texture per capture and, when the renderer supports fences, copies the pixels once the GPU is done instead of waiting; the copy itself still runs on the main thread ([#109](https://github.com/Deliveos/nori/pull/109); fixes [#30](https://github.com/Deliveos/nori/issues/30)).

## Memory

- **Blur and xray buffers.**
    niri keeps each output's blur/xray effect buffers (an output-sized texture plus a blur chain for each render target) for as long as the output exists; the issue estimates about 465 MB on a 4K output after a blurred window, the overview, a screencast and a screenshot.
    nori frees buffers that no frame has used for 10 seconds and creates them again when needed; buffers that a window's unmap snapshot still references (it is drawn when the window closes) are kept ([76e7a0fe](https://github.com/Deliveos/nori/commit/76e7a0fef9a735f31f8f9d09fd9a22ae01c1db83), [f8350b40](https://github.com/Deliveos/nori/commit/f8350b4019a17ca2deb995ea0d1b19a68c8a75d8); fixes [#25](https://github.com/Deliveos/nori/issues/25)).
- <a id="pipewire-buffers"></a>**PipeWire buffers.**
    niri offers PipeWire consumers 8 buffers by default, each a whole frame, about 265 MB per 4K stream.
    nori offers 4 by default and still allows 2–16 for consumers that ask; the default is not configurable.
    Consumers without a preference of their own, such as OBS, get 4 ([#106](https://github.com/Deliveos/nori/pull/106); fixes [#33](https://github.com/Deliveos/nori/issues/33)).
- **Hotkey overlay textures.**
    niri keeps the hotkey overlay's texture for each output after the overlay is closed, and the overlay is shown at startup by default; the issue estimates about 10 MB per output at scale 2.
    nori frees them when the overlay is hidden ([96708679](https://github.com/Deliveos/nori/commit/96708679522003d8d4eb443169e64cd147617647); fixes [#42](https://github.com/Deliveos/nori/issues/42)).
- **Closing windows.**
    For every closing window, niri also renders its blocked-out contents into a second window-sized texture (33 MB for a fullscreen 4K window), which is only used with a `block-out-from` rule.
    nori does this only for windows with such a rule ([93ffe3bc](https://github.com/Deliveos/nori/commit/93ffe3bc61b0f084caa38da2f17506ecd591b47f), [e9b214ff](https://github.com/Deliveos/nori/commit/e9b214ff5baf6d82e06ae451993e71322a0fefd3); fixes [#36](https://github.com/Deliveos/nori/issues/36)).
- **Screenshot UI and screen transitions.**
    niri renders every output three times, once per render target, when the screenshot UI opens or a screen transition starts.
    Without `block-out-from` rules the three are identical, so nori renders each output once and shares the texture ([#67](https://github.com/Deliveos/nori/pull/67); fixes [#35](https://github.com/Deliveos/nori/issues/35)).
- **Layer surfaces of unplugged outputs.**
    niri keeps an entry for each layer surface (bar, wallpaper) of an output that is removed, one more on each unplug.
    nori removes them with the output ([0280b8b2](https://github.com/Deliveos/nori/commit/0280b8b2f20e84788b614db46d6face684804b44); fixes [#18](https://github.com/Deliveos/nori/issues/18)).
- **ScreenCast D-Bus sessions.**
    niri keeps every ScreenCast D-Bus session it ever created in a list that is never read.
    nori removes that list; the same commit removes other dead code ([63f4474e](https://github.com/Deliveos/nori/commit/63f4474ee4acf4a0bff44844ee3e0cbdadbd3059); part of [#24](https://github.com/Deliveos/nori/issues/24)).

## Building and packaging

- **smithay fork.**
    niri builds smithay and smithay-drm-extras from [Smithay/smithay](https://github.com/Smithay/smithay) at `8a270232`.
    nori builds them from [Deliveos/smithay](https://github.com/Deliveos/smithay) at `35cfc75f`, on its `nori` branch: the same version plus the `xdg_positioner` fixes for [#90](https://github.com/Deliveos/nori/issues/90) and [#104](https://github.com/Deliveos/nori/issues/104).
    Builds that fetch or vendor the dependencies, including the Nix flake, get smithay from there, so the pinned fork commits have to stay reachable from a branch or tag of the fork ([#101](https://github.com/Deliveos/nori/pull/101), [#122](https://github.com/Deliveos/nori/pull/122)).
    Switching back to upstream smithay once the fixes are there is tracked in [#105](https://github.com/Deliveos/nori/issues/105).
- The renamed packaging files and Nix flake attributes are listed under [Names](#names).

## Tests and CI

- Client-server regression tests for the crash and leak fixes ([4a000d50](https://github.com/Deliveos/nori/commit/4a000d504c9ab0fd8b7c731a004a90a68b353dd7), later extended), and a fix for one of them that failed when run with other tests ([#103](https://github.com/Deliveos/nori/pull/103); fixes [#102](https://github.com/Deliveos/nori/issues/102)).
- Tests for foreign-toplevel close detection ([#66](https://github.com/Deliveos/nori/pull/66); fixes [#53](https://github.com/Deliveos/nori/issues/53)) and for the pointer inactivity timer ([#87](https://github.com/Deliveos/nori/pull/87); fixes [#82](https://github.com/Deliveos/nori/issues/82)).
- Screenshot tests that need EGL carry the `egl_` prefix, so the Nix sandbox skips them ([#91](https://github.com/Deliveos/nori/pull/91); fixes [#79](https://github.com/Deliveos/nori/issues/79)), and the nix CI job fails when `nix flake check` fails; niri lets that job pass anyway ([#121](https://github.com/Deliveos/nori/pull/121); fixes [#97](https://github.com/Deliveos/nori/issues/97)).
- CI runs once per pull request commit: pushes start CI only on `main`, and a newer commit cancels the older run ([#2](https://github.com/Deliveos/nori/pull/2)); AGENTS.md and CONTRIBUTING.md describe when CI runs here ([#2](https://github.com/Deliveos/nori/pull/2), [#46](https://github.com/Deliveos/nori/pull/46)).

## Project rules

niri doesn't accept code, issues or pull request descriptions written by LLMs.
nori welcomes people and AI agents under the same rules: [AGENTS.md](AGENTS.md), [CONTRIBUTING.md](CONTRIBUTING.md) and the pull request template ([95e89dc0](https://github.com/Deliveos/nori/commit/95e89dc043105db77c0b614b5c4a76645e6ba544)).
Agents never review, approve or merge a pull request with commits they wrote; another agent or a person merges it once CI is green and no review threads are unresolved ([#61](https://github.com/Deliveos/nori/pull/61), which replaced the rule from [bb073b08](https://github.com/Deliveos/nori/commit/bb073b08e21110bf504f395aa334a907ac57ef1a) that let agents merge their own; fixes [#60](https://github.com/Deliveos/nori/issues/60)).
The issue templates, the README and the contributing guides point at this repository ([#4](https://github.com/Deliveos/nori/pull/4), [#48](https://github.com/Deliveos/nori/pull/48), [#78](https://github.com/Deliveos/nori/pull/78); fixes [#47](https://github.com/Deliveos/nori/issues/47), [#76](https://github.com/Deliveos/nori/issues/76)).

## Upstream merges

nori takes upstream niri in with merge pull requests, so upstream's history is kept.
Upstream's own changes are not listed in this file.

| Pull request | Upstream commits | Merged |
|---|---|---|
| [#117](https://github.com/Deliveos/nori/pull/117) | [e19df12](https://github.com/niri-wm/niri/commit/e19df12cf81ffe805609fee497da1a097066ad2f)..[9d8f4f8](https://github.com/niri-wm/niri/commit/9d8f4f89f6a075a04ed24af5f13829d0bdba5b85) (18 commits) | 2026-10-26 |

To see the upstream commits that nori doesn't have yet, run `git fetch https://github.com/niri-wm/niri main` and then `git log --oneline HEAD..FETCH_HEAD`.

## Open problems

Open issues in nori's own changes:

- [#105](https://github.com/Deliveos/nori/issues/105): keep the pinned smithay fork commits reachable, and switch back to upstream smithay.
- [#124](https://github.com/Deliveos/nori/issues/124): unused import warning in builds without D-Bus ([#125](https://github.com/Deliveos/nori/pull/125)).
- [#58](https://github.com/Deliveos/nori/issues/58): keep the performance benchmarks in the repository.

Problems found in upstream niri, or in the smithay version it uses, while working on nori, not fixed yet: [#24](https://github.com/Deliveos/nori/issues/24) (the rest of it), [#119](https://github.com/Deliveos/nori/issues/119), [#128](https://github.com/Deliveos/nori/issues/128) (smithay).
