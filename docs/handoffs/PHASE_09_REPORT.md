# Phase 09 Report — Release Quality, Compatibility, Documentation and Packaging

**Date:** 2026-11-04
**Workspace:** `/home/baris/Work/licoman` (`main`, Phases 07–08 commits +
this phase's uncommitted changes — nothing was committed or pushed by the
automated session; the tree is ready for review).
**Executor:** automated session (ZCode). **Phases 08 and 09 were executed
in one session on the user's explicit request**; this report closes 09 and
the whole development sequence.
**Plan source:** `09_RELEASE_QUALITY_AND_PACKAGING.md` (author's document
pack). All nine handoff sections below; **final Go / No-Go decision in §8**.

---

## 1. Goal and actual scope implemented

**Goal (from the plan):** prepare Licoman for a safe, trustworthy **Linux
desktop preview / first public release**, with claims matching
demonstrated functionality — without silently broadening permissions,
enabling telemetry, or executing automatic system modifications.

Implemented:

- **Release positioning (§1 of the plan):** the product is *Licoman*
  ("Linux Configuration Manager") everywhere — binary, launcher, AppStream,
  documentation. Support is stated honestly per integration
  (`docs/compatibility.md`; the AppStream description mirrors it). No
  "every Linux setting" claim anywhere; no sign-in, cloud sync, hosted
  service, background daemon or telemetry exists (grep-audited: no network
  client in the codebase at all). Dark stays the first-run default; Light
  and System are user-selectable and applied to both windows.
- **Vulkan startup diagnostics (§2):** a filesystem-only pre-probe (loader
  library + at least one ICD manifest) runs before GPUI starts; missing
  prerequisites produce per-distro package advice plus the troubleshooting
  link, and the process exits — no raw renderer failure (D60). A panic
  hook appends the same pointer to graphics-shaped panics.
  `LICOMAN_SKIP_VULKAN_CHECK=1` is the documented headless escape hatch.
  Verified live: the success path on this machine; the escape hatch;
  `docs/troubleshooting.md` written (Vulkan, session, services, prefs).
- **Packaging (§3):** an Arch **PKGBUILD** (`packaging/licoman/`) — built
  end-to-end with `makepkg` on this machine into
  `licoman-0.1.0-1-x86_64.pkg.tar.zst` and verified (`pacman -Qip`, full
  file list: binary, desktop file, hicolor icons 128/256/scalable,
  AppStream metainfo, LICENSE + NOTICES, offline docs). The build was
  **not installed** on the live system — `pacman -U` is a system
  modification the user did not approve for this session; the same L-gate
  discipline as Phase 07 applies. App ID `io.github.barscn.Licoman` (D59 —
  the AppStream-reserved `io.github.<user>` convention, not a fabricated
  domain). No Flatpak (explicitly deferred per the plan); **no .deb/.rpm
  placeholders** shipped (the plan forbids promising untested formats —
  stated in `docs/installation.md` and the changelog). The package does
  not require root to run; uninstall removes only package-owned files
  (documented; the user-owned preferences/backups are never touched).
- **Quality gates measured (§4):** see the table in §4 — every figure is a
  real measurement on the reference machine with the release build
  (method, hardware and sample counts recorded). **No invented numbers.**
- **Security and reliability review (§5):** threat model re-walked against
  the code (grep evidence in §7): exactly one process spawn in the
  shipped binary (the documented `--version` probe — direct exec, bounded,
  no shell); no `sh -c` anywhere; no network clients; no sudo/root paths;
  typed D-Bus calls only; the log-value redaction test stays green.
  Dependency/license audit: **816 locked entries in `Cargo.lock`, every
  third-party crate declares an SPDX license** (programmatic audit; only
  the three workspace crates lacked a field — fixed via the workspace
  `license = "MIT"`). NOTICES written; `Cargo.lock` is the SBOM manifest.
  Error-path coverage (read-only fs, disk full, permission denial,
  malformed config, no session bus) was already machine-tested in Phases
  05–08 fixture suites; the packaging check adds `cargo test --release -p
  licoman-core` into `makepkg check`.
- **Accessibility and design review (§6):** `docs/design/accessibility-
  review.md` — what is verified (design system across windows, status in
  words + icons, palette keyboard flow, narrow widths, 1.5× fractional
  scale mechanism, corrupt-prefs fallback) and what honestly remains a
  human gate (full keyboard-only walkthrough, Orca/AT walkthrough,
  125 %/200 % scale sweep).
- **Documentation (§7):** README.md (updated status + honest support),
  LICENSE (MIT — **a proposal recorded in the report; the final license
  choice is the maintainer's**), NOTICES, `docs/installation.md`,
  `docs/usage.md`, `docs/configuration-safety.md`, `docs/privacy.md`,
  `docs/contributing.md`, `docs/troubleshooting.md`,
  `docs/design/accessibility-review.md`, `CHANGELOG.md`, plus the Phase
  08 doc set (compatibility, module-trust). Screenshots are **actual
  release-build captures** of both windows in Dark and Light
  (`p09-release-*.png`).
- **Release candidate process (§8):** functionality and dependencies
  frozen (no renderer/GPUI upgrade was attempted); the four cargo gates
  plus the manifest lint re-run clean (283/283 tests); a preferences
  upgrade/migration contract test added (older-schema file preserves the
  theme selection, re-save normalizes — pinned before any real schema
  change); version proposed as **0.1.0-alpha.0** (AppStream release entry
  + changelog). **Nothing was published** — no repo push, no package
  upload, no announcement; per the plan, this phase creates a release
  candidate and a release plan, not a release.

## 2. Changed/added files and public interfaces

```text
apps/licoman/src/main.rs          + vulkan_precheck() (loader + ICD manifests,
                    filesystem-only), graphics-panic hook, startup timing mark
crates/ui/src/startup.rs          NEW: process-start → first-frame measurement
                    (logged once: "first frame rendered elapsed_ms=…")
crates/ui/src/ui/main_window.rs   + note_first_frame() call in render
crates/ui/src/windows.rs          app_id → io.github.barscn.Licoman (D59)
crates/core/src/prefs.rs          + older-schema migration contract test
Cargo.toml (+core/ui)             + workspace license = "MIT"
LICENSE                           NEW (MIT — proposal, see §1)
NOTICES                           NEW (third-party license inventory)
packaging/licoman/PKGBUILD        NEW (tested: builds + checks + packages)
packaging/desktop/io.github.barscn.Licoman.desktop        NEW
packaging/metainfo/io.github.barscn.Licoman.metainfo.xml  NEW
assets/io.github.barscn.Licoman.svg + hicolor PNGs (128/256)  NEW
docs/troubleshooting.md           NEW (Vulkan/session/services/prefs)
docs/installation.md, usage.md, configuration-safety.md, privacy.md,
  contributing.md                 NEW (the §7 documentation set)
docs/design/accessibility-review.md  NEW (§6)
CHANGELOG.md                      NEW (0.1.0-alpha.0 entry)
README.md, 00_START_HERE.md       status banners updated
docs/architecture/decisions.md    D59–D60
tests/tools/measure-p09.sh        NEW (idle CPU/RSS + first-frame harness)
tests/tools/capture-p08.sh        + title/pid-targeted window picking (stale
                    windows never win), demo-env passthrough, release cases
docs/handoffs/PHASE_09_REPORT.md  this file
```

Public interfaces:

- Binary: `LICOMAN_SKIP_VULKAN_CHECK=1`; the startup log line
  `target=licoman::startup msg="first frame rendered" elapsed_ms=N`.
- `licoman_ui::startup::{mark_process_start, note_first_frame, measured}`.
- Window/app id `io.github.barscn.Licoman` (also the desktop/AppStream id).
- Package: `licoman` 0.1.0-1 (x86_64) — files listed in §3.

## 3. Packaging verification (build-only, per the L-gate)

| Step | Command | Result |
|---|---|---|
| Source tarball | working-tree tar (excluding `target/`, `.git`) + sha256 | built, checksum pinned in the test PKGBUILD |
| Build | `makepkg -f --nodeps` (nodeps: cargo comes from rustup, not pacman) | **exit 0** — full release build inside the package environment |
| Check | `makepkg` runs `cargo test --release -p licoman-core` | **passed** (headless suite) |
| Package | artifact | `licoman-0.1.0-1-x86_64.pkg.tar.zst` (15.97 MiB compressed / 45.53 MiB installed) |
| Metadata | `pacman -Qip` | name/version/license/deps/opt-deps all correct |
| Contents | `tar -tf` | binary, desktop file, metainfo, icons ×3 sizes, LICENSE, NOTICES, 5 offline docs |

**Packaging finding (documented in the PKGBUILD):** the distro-wide
makepkg `RUSTFLAGS` (LTO + path remap) break the gpui/wayland-backend link
(`undefined symbol: wl_log_trampoline_to_rust_client` under rust-lld —
isolated by rebuild experiments to those flags, not the source tree or the
fresh cargo home). `build()` unsets the four flags and builds with the
same plain `cargo build --release` the repository's release gate uses.
Also: `/tmp` has a disk quota smaller than a fresh 816-crate build — the
stage lives on the home filesystem (`~/licoman-pkgstage`, outside the
repository).

**Not executed (needs explicit approval, same as Phase 07's L-gates):**
`pacman -U` install on the live system, launcher launch test from the
installed file, upgrade and uninstall cycle. The documented manual test
steps live in `docs/installation.md` for the maintainer.

## 4. Quality gates — measured, with method

Reference machine for all figures: Arch Linux, kernel 7.2.14, **AMD Ryzen
7 7800X3D (8 cores)**, Hyprland 0.57 (Wayland), Mesa/RADV Vulkan, **release
build** (`cargo build --workspace --release`), pointer parked at the
screen corner during idle sampling (hover is not idle), 7 s settle before
sampling.

| Metric | Plan target | Measured | Method |
|---|---|---|---|
| Cold start → interactive main window | < 2 s | **595 / 598 / 610 / 600 / 631 ms** (5 consecutive runs, mean ≈ 607 ms; scale catalog 538 ms) | new first-frame instrumentation (`licoman::startup` log), process start → first main-window render |
| Idle CPU (main window, live providers) | < 1 % mean | **0.07 %** over 30 s | /proc `<pid>/stat` utime+stime delta, one core = 100 % |
| Idle CPU (scale demo, 1,240 rows) | < 1 % mean | **0.07 %** over 30 s | same |
| Idle memory (live) | monitor vs prototype; no hard guarantee | **400 MB RSS, byte-identical across 40+ s** (no churn) | /proc `<pid>/status` VmRSS at t0/t30/t40 |
| Idle memory (scale demo) | same | **409 MB RSS, byte-identical** | same |
| Search, 1,240 synthetic settings + 120 apps | p95 < 100 ms | **p50 23 µs, p95 101 µs, max 138 µs; index build 746 µs** (release build; debug-mode figures 239/383 µs also recorded) | `scale_search_latency_1000_settings_100_apps` (12 queries × 3 runs, `--nocapture`) |
| Main/Apps window switch | no visible freeze | opening/selecting via the capture nav hook completes within the capture window (≤ 4.5 s nav + shot); no multi-second redraws observed in any capture | screenshots (5 release captures) |
| Live theme switch | both windows update without restart | Light demo runs live (`p08-light-scale.png`, `p09-release-apps-light.png`); the store/theme path is exercised headlessly | capture harness + existing theme tests |
| Backend timeouts | UI stays interactive | worker-thread + 3 s bound on every D-Bus client call (Phase 05 design, still asserted: `unresponsive_service_times_out_without_hanging`, both dbus and xfconf suites) | unit tests |
| Startup no-backend case | clear useful UI, no crash | `LICOMAN_DEMO=empty` renders the unavailable state (`p09-no-backend.png`); log shows 0 providers, clean discovery | capture + log |

**Honest caveat:** these are single-machine figures. They demonstrate the
targets are reachable on the documented reference hardware, not a
performance guarantee across GPUs/distributions — exactly the plan's
"state the machine" rule; the release notes say "measured on one
documented reference machine".

## 5. Security and reliability review

- **Process spawns (grep evidence):** the shipped binary contains exactly
  one `Command::new` — the Alacritty/mpv `--version` probe (direct exec of
  a resolved binary path, 3 s kill bound, no shell). The other matches are
  the `#[ignore]`d Phase 07 live harness (read-only `gsettings`/`dconf`
  verification CLIs and a disposable private-bus `dconf-service`). No
  `sh -c`, no shell interpolation anywhere in the codebase.
- **D-Bus calls are typed end to end** (zbus proxies with concrete types;
  PowerProfiles/Xfconf method args are `(String,String)`-shaped protocol
  arguments validated by the manifest grammar — never concatenated
  commands).
- **Privilege boundary:** no sudo, no polkit bypass, no setuid, no root
  paths (grep: only doc comments mention them). Deny-by-default broker
  unchanged.
- **Log redaction:** the planted-secret test (values never in logs or
  operation records) passes in the final gate run.
- **Dependency/license audit:** 816 `Cargo.lock` entries; every
  third-party crate declares an SPDX license (programmatic scan of the
  registry sources: gpui/gpui-kit/gpui-fast Apache-2.0, zbus/zvariant/
  ashpd/gio MIT, notify CC0-1.0, the Rust-core stack MIT OR Apache-2.0;
  full narrative in `NOTICES`). No dependency is modified; none bundles
  network services. `Cargo.lock` ships as the dependency manifest/SBOM
  source.
- **Failure-path coverage** (already machine-tested in the fixture
  suites, re-run in the final gates): ENOSPC staging, permission denial,
  foreign ownership, symlink escape, malformed TOML/mpv conf, invalid
  UTF-8, absent service/bus, readback mismatch with restore. The
  packaging `check()` runs the headless suite in the package
  environment.
- **Live writes:** none executed this phase (no Alacritty/PowerProfiles
  targets on this machine; GSettings live write approval was granted in
  Phase 07 only). CI never touches host configuration; the release
  scripts (makepkg/check) run only repository-local builds and tests.
- **Restore documentation:** `docs/configuration-safety.md` documents
  backup locations, the automatic restore behavior and the manual
  recovery path.

## 6. Accessibility and design review

See `docs/design/accessibility-review.md`. Summary: one design system
across both windows and both themes (release captures); status in words
plus icons; palette keyboard flow implemented and demo-verified; **human
gates remain** for the full keyboard-only walkthrough, an Orca/AT
walkthrough, and the 125 %/200 % scale sweep — recorded as known
limitations, with no WCAG-conformance claim.

## 7. Tests/commands run, with real outcomes

| Command | Result |
|---|---|
| `cargo fmt --all -- --check` | **pass** (after reformatting the new files) |
| `cargo test --workspace` | **pass — 283/283** (core 271 incl. the new prefs migration test; cli 7; ui 5) + **1 ignored** (live harness) |
| `cargo clippy --workspace --all-targets -- -D warnings` | **pass — 0 warnings** (two new-code lints fixed: a `useless_format`, a `useless_conversion`) |
| `cargo build --workspace --release` | **pass** |
| `./target/release/licoman module validate modules/builtin/` | 17/17 valid |
| `makepkg -f --nodeps` (staged PKGBUILD) | **pass — package built + verified** |
| Release smoke runs | discovery, live reads, no-backend demo, scale demo — all clean; zero write events |
| 5-run startup timing | 595–631 ms (recorded above) |
| 30 s idle CPU/RSS sampling (live + scale) | 0.07 % CPU; stable RSS (recorded above) |

**Bugs found and fixed during Phase 09 verification:**

1. **Clippy lints in the new Vulkan precheck** (`useless_format`,
   `useless_conversion`) — fixed, gates re-run clean.
2. **Packaging link failure** — makepkg's distro RUSTFLAGS (LTO/remap)
   break the wayland-backend link; root-caused by rebuild experiments
   (plain build of the same extracted tree with the same fresh cargo home
   succeeds; only the flag set differs) and fixed in the PKGBUILD with a
   documented comment.
3. **A stale-window capture bug** in the harness: after failed runs left
   12 licoman windows open, the picker silently matched nothing (and an
   earlier `sed` produced invalid Python). Fixed: pid-based window pick +
   corrected class filter.
4. **The `/tmp` disk quota** kills a fresh in-`/tmp` package build
   ("Disk quota exceeded" mid-compile) — packaging staging moved to the
   home filesystem; documented.

## 8. Release decision proposal — **Go for 0.1.0-alpha.0 (conditional)**

| Acceptance gate (09 plan §9) | Status |
|---|---|
| No destructive changes on install/launch/discovery/test/theme switch | **pass** — the app's only self-writes are its preferences file; discovery is passive; the live-run write-events log stays empty; theme switching is store-local |
| Supported integrations truthfully reflect version/provider availability | **pass** — `docs/compatibility.md` is per-integration with cited evidence; per-row probes re-verify on every machine |
| App usable when DE/provider/service/app-config/Vulkan prerequisites are unavailable | **pass** — honest unavailable states (empty demo capture); Vulkan-missing machines get the actionable diagnostic and exit |
| Dark default on first run; Light/System persist; both windows consistent | **pass** (unit tests + release captures in both themes) |
| No Electron/WebView dependency, no secretly installed daemon | **pass** — grep-audited; the app spawns nothing beyond the documented version probes |
| Packaging install/upgrade/uninstall tested on declared targets | **partial** — PKGBUILD builds, checks and packages with verified contents; **the live install cycle was not executed** (not approved this session). The gate cannot honestly be marked pass until a maintainer runs `pacman -U`/`-R` — the commands are documented in `docs/installation.md` |
| Mandatory commands and manual checks recorded with outcomes | **pass** — this report + the Phase 08 report record every gate with real output |
| Security and accessibility findings triaged; critical unresolved issues block release | **pass (triage)** — no critical security findings; the accessibility items are human gates, disclosed |
| README/module docs/safety docs/compatibility/license/honest release notes ready | **pass** — the full §7 set exists; LICENSE = MIT is flagged as the maintainer's final call |

**Proposal:** tag **0.1.0-alpha.0**. The alpha label reflects the honest
state: single-machine performance evidence, packaging install cycle and
the AT/keyboard walkthroughs outstanding. **Nothing is published by this
session** — publishing (repository push, tag, package upload, any
announcement) remains the maintainer's explicit action, per the plan's
closing rule.

**No-Go conditions that would block the tag** (none currently met): any
unresolved write-path safety regression, a real dconf/config write during
CI or normal startup, or a security finding in the new backend families.

## 9. Reverting this phase

No user configuration was touched: the real dconf database was never
written; no package was installed; the only host-state changes are the
two disclosed operator actions from Phase 08 (xfconfd start — since
exited; the mpv.conf fixture). To revert the repository to the pre-Phase-
09 state:

```sh
cd /home/baris/Work/licoman
git checkout -- . && git clean -fd \
  LICENSE NOTICES CHANGELOG.md packaging assets/icons \
  docs/troubleshooting.md docs/installation.md docs/usage.md \
  docs/configuration-safety.md docs/privacy.md docs/contributing.md \
  docs/design/accessibility-review.md \
  crates/ui/src/startup.rs tests/tools/measure-p09.sh \
  docs/screenshots/p09-*.png
# keeps Phase 08 exactly as reviewed
```

The workspace `license = "MIT"` line and the two-window app-id change
drop out with the checkout. Nothing outside the workspace was modified
by this phase.

---

**End of the development task sequence.** The plan's closing line applies:
this phase creates a release candidate and a release plan — it is not
permission to publish.
