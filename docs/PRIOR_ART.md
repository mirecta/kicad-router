# Prior art survey (M0)

**Status:** required M0 deliverable per `AUTOROUTER_PLAN.md` §1/§10. Written
2026-07-28 against KiCad 10.0.3. Every project below was actually cloned and
read (file paths, function/class names cited), not summarized from README
impressions alone, per the plan's rule zero ("verify, don't assume").

---

## Topola (`codeberg.org/topola/topola`) — Rust

**What it is.** A Rust workspace (root crate `topola`, plus
`topola-cli`, `topola-egui`, `specctra-core`/`specctra_derive`,
`planar-incr-embed`) implementing an interactive rubber-band router with a
real autorouting mode on top. It genuinely is the topological approach the
plan wants for `tessera-detail`'s M4 phase: `src/triangulation.rs` wraps
`spade::ConstrainedDelaunayTriangulation` with an arena-of-indices design
(`FixedVertexHandle`, custom `GetIndex` node ids) — not a C++-style pointer
graph — and `src/router/navmesh.rs` + `src/router/thetastar.rs` route via
Theta*/A* over a navigation mesh built on that CDT. `src/drawing/drawing.rs`
(1106 lines, the largest file) manages the actual rubber-band geometry.

**Autorouting maturity.** Both interactive (`Autorouter::pointroute`) and
batch (`planar_autoroute`, `multilayer_autoroute`, `topo_autoroute`) paths
exist, with a real headless CLI (`topola-cli`: load DSN → `Autoroute` command
→ write `.ses`). But the test suite still drives autorouting via
hand-recorded per-net JSON scripts rather than "point at an arbitrary board
and it completes," and there's a second, apparently-in-progress routing
engine (`router::ng`, `planar-incr-embed`) alongside the original — a sign the
core isn't considered finished even internally.

**KiCad integration.** File-based only, via Specctra DSN/SES
(`src/specctra/design.rs`, `crates/specctra-core`) — zero IPC (`grep -ri ipc`
across the tree: no hits). DRC modelling is minimal: a single
generic clearance/width per net class (`SpecctraRule{width, clearance}` in
`crates/specctra-core/src/mesadata.rs`), no custom-rule expression language,
no diff pairs anywhere (`grep -ri "diff.?pair"`: zero hits), no locked-item
or protected-region concept, keepouts parsed from DSN but not wired into
obstacle avoidance.

**Maturity/activity.** ~28k LOC (excl. tests), 1365 commits since
2023-07-12, ~15 contributors, REUSE/SPDX-compliant, has proptest and a fuzz
target. But commit cadence tells a cautionary story: 74 commits in Oct 2025,
50 in Nov 2025, then a cliff to 1-2/month since — essentially dormant for
~8 months as of this writing. Rough edges present (`todo!()` in undo paths,
`#![allow(unused_must_use)]` at crate root).

**License.** MIT, confirmed (`LICENSES/MIT.txt`, SPDX headers throughout).

**Governance.** NLnet-funded (NGI0 Entrust Fund, grant 101069594), genuinely
open to contribution (`CONTRIBUTING.md`: "*Anyone* can contribute... under
any identity"), Matrix/IRC chat, explicit AI-contribution policy.

### Recommendation: build tessera fresh, do not contribute to or merge with Topola

Reasoning:

1. **Scope mismatch, not just license.** Topola has essentially none of
   tessera's core scope: no custom-DRC-rule engine, no diff pairs, no
   protected regions/locked items, no IPC. These aren't small gaps to
   contribute — they *are* most of what tessera exists to build. "Contributing"
   would mean building tessera's DRC/IPC/diff-pair/region machinery inside
   Topola's codebase — a rewrite-in-place, not a contribution.
2. **The one-way GPL/MIT door makes this worse, not neutral.** Tessera
   (GPL-3.0-or-later) can freely absorb Topola's (MIT) code today. If instead
   we built the missing DRC/IPC/diff-pair machinery as upstream contributions
   to Topola, that code would have to be released under MIT — permanently
   forfeiting the option to have built it under GPL, and handing a
   permissively-licensed project exactly the hard, differentiating parts
   (DRC parity, IPC bridge) tessera is meant to own. Building fresh keeps all
   optionality open in one direction only.
3. **Momentum risk.** Betting a "join forces instead of building" decision on
   a project whose commit pace collapsed 8 months ago is a real risk,
   independent of code quality.
4. **What to take, not join.** Topola's `spade`-CDT + arena-of-indices +
   Theta*-over-navmesh design validates that this exact architecture works in
   this exact domain (PCB routing, not IC routing) — read closely as a design
   reference for tessera's M4 topological router, cited in
   `docs/PROVENANCE.md` if code or structure is ever directly drawn from it,
   but not imported wholesale (tessera's constraint model is structured too
   differently, and per plan §11.3 nothing gets transliterated regardless).

---

## KiCadRoutingTools (`github.com/drandyhaas/KiCadRoutingTools`) — Rust + Python

**Correction to the plan's framing:** this is not "Rust talks to Python over a
channel" — Rust is a native Python extension via PyO3
(`rust_router/Cargo.toml`: `crate-type = ["cdylib"]`, `#[pymodule] fn
grid_router`), imported in-process as `import grid_router`. There is no
subprocess/socket boundary between the two languages; the boundary is an FFI
call.

**Packaging (directly reusable template for tessera §8.4).** `metadata.json`
at repo root is a standard KiCad PCM manifest. `package_pcm.py` stages the
whole repo under `plugins/`, drops in **prebuilt Rust binaries for all four
platforms** (`grid_router-{linux-x86_64,macos-arm64,macos-x86_64,windows-x86_64}.{so,pyd}`)
under `rust_router/`, and a root `__init__.py` resolver picks the right binary
by `sys.platform`/arch at runtime. One zip, all platforms, thin picker stub —
exactly the shape tessera's `tessera-plugin` packaging should follow.

**IPC path — a real correction to the plan's assumption.** It does **not**
use KiCad's modern IPC API at all (zero `kipy`/protobuf references anywhere).
GUI mode subclasses `pcbnew.ActionPlugin` and runs in-process inside KiCad's
embedded Python via the legacy **SWIG** `pcbnew` module — exactly the
unstable-ABI, no-crash-isolation path the plan's §2.1 deliberately rejects.
A separate CLI mode (`route.py`) parses/writes `.kicad_pcb` S-expressions
directly, which is the genuinely external-process path and matches tessera's
file-adapter fallback. DRC verification shells out to `kicad-cli pcb drc
--format json` as a subprocess oracle — validating that same approach for
tessera's own parity harness (§3.2).

**Algorithm.** Confirmed grid octilinear A* (0.1mm grid, 8-directional +
via-layer moves, 3D Chebyshev heuristic), cell-based obstacle map computed by
rasterizing exact geometry in Python and handing flat arrays to Rust — a
real architectural compromise tessera's exact-geometry kernel (`tessera-geom`)
is meant to avoid entirely.

**Diff pairs — the most transferable idea here.** `rust_router/src/pose_router.rs`
implements pose-based A* over state `(gx, gy, theta_idx, layer)` with an
admissible **Dubins-path heuristic** (`dubins.rs`, all 6 CSC/CCC path types),
minimum-turn-radius constraints, cumulative-turn caps, and inline GND-return-via
site search evaluated *during* the search rather than as a post-pass. This is
the concrete mechanism the plan's §6 gestures at for the grid-baseline diff
pair router.

**DRC-parity approach — weaker than tessera's bar.** No independent DRC
reimplementation; relies on iterating `kicad-cli pcb drc` until clean
(`kicad_oracle.py`: run DRC → parse unrouted/violating items → route exactly
those → repeat). Real and working, but this is "converge against KiCad DRC
after the fact," not tessera's non-negotiable "prove parity before routing."

**Maturity.** Very young (7.5 months) but extremely active (2081 commits,
768 in the last ~28 days), 205 test files plus a stress-test harness grading
against ~25 real open-hardware boards. Substantially AI-agent-driven
development per its own `.claude/skills/`.

**Borrow (conceptually — never port code, per plan §11.3):** the PCM
packaging shape; pose-based A* + Dubins heuristic as the diff-pair mechanism
for tessera's M2 grid baseline; `kicad-cli pcb drc` as a continuous
cross-check even after `tessera-drc` exists; its frontier-attribution rip-up
scheduler design (multiple ranking heuristics, ripped-corridor soft cost,
history-based termination) as a reference for `tessera-engine`.

**Avoid repeating:** SWIG in-process GUI integration (no crash isolation, no
stable ABI — exactly what tessera's IPC-first architecture is meant to fix);
treating "DRC eventually converges" as equivalent to parity; single-cell
(no-rotation) obstacle grids that push geometry correctness into ad hoc
Python rasterization.

---

## KiCad PNS (`pcbnew/router/`) — C++, GPL-3.0-or-later

Read via sparse checkout (`pcbnew/router` only) of `gitlab.com/kicad/code/kicad`.
**90 files, 40,047 LOC** — not a small subsystem, but the part that matters for
`tessera-drc` is concentrated in a handful of headers, not the full tree.

**Confirms the plan's "interactive, not batch" framing precisely.** The core
state object `PNS::NODE` (`pns_node.h`/`.cpp`) is explicitly a
copy-on-write branch/merge tree (`NODE::Branch()`, `NODE::Commit()`) built to
support "try a shove, discard it, revert" within a single mouse-move tick.
`PNS::SHOVE` has a hard **1000ms time limit** (`m_shoveTimeLimit`,
`pns_routing_settings.cpp:44`) checked mid-algorithm — an interactive latency
budget baked directly into the algorithm's control flow, not a tunable batch
parameter. `router_tool.cpp`'s tool loops are literal
`while (TOOL_EVENT* evt = Wait())` pumped by mouse-motion events. This is
exactly why the plan says (§11.3) "do not port PNS" — its shape is correct
for cursor-latency shove, and wrong for batch throughput.

**What it tells us empirically about KiCad's rule semantics (the useful
part).** `RULE_RESOLVER` (`pns_node.h`) is the abstract interface PNS uses to
query KiCad's real rule engine: `Clearance()`, `QueryConstraint(CONSTRAINT_TYPE, ...)`,
`DpCoupledNet`/`DpNetPolarity`, `IsKeepout`, `IsNetTieExclusion`,
`IsDrilledHole`. Its `CONSTRAINT_TYPE` enum is a ready-made checklist for
`tessera-drc`'s §3.1 coverage, taken from the shipping product rather than
inferred: `CT_CLEARANCE`, `CT_DIFF_PAIR_GAP`, `CT_LENGTH`, `CT_WIDTH`,
`CT_VIA_DIAMETER`, `CT_VIA_HOLE`, `CT_HOLE_CLEARANCE`, `CT_EDGE_CLEARANCE`,
`CT_HOLE_TO_HOLE`, `CT_DIFF_PAIR_SKEW`, `CT_MAX_UNCOUPLED`,
`CT_PHYSICAL_CLEARANCE`, `CT_PHYSICAL_HOLE_CLEARANCE`. `SHOVE_STATUS`
(`SH_OK`/`SH_INCOMPLETE`/`SH_HEAD_MODIFIED`/`SH_TRY_WALK`) documents the
shove → walk-forward/back → ignore fallback ladder, a useful behavioral
reference even though tessera's own implementation must differ.

**Feasibility of "read, don't port."** Fully feasible: the constraint surface
and shove/walkaround state machines live in a handful of headers
(`pns_node.h`, `pns_shove.h`, `pns_walkaround.h`, `pns_router.h`) that can be
read without following the interactive event-loop plumbing
(`router_tool.cpp`, `pns_dragger.cpp`), which can be skipped almost entirely.

---

## toporouter / pcb-rnd — C

`repo.hu` (the canonical pcb-rnd host) is **unreachable from this network**
(`curl` to `https://repo.hu:443`: connection refused, verified directly, not
assumed). Found and read a working mirror instead: `github.com/russdill/pcb`,
a gEDA PCB fork carrying the original 2009 toporouter.

**License, verified against actual file headers, not a project summary
page:** `src/toporouter.c:11` and `src/toporouter.h:10-11` both read *"the
Free Software Foundation; either version 2 of the License, or (at your
option) any later version"* — **GPL-2.0-or-later**, the favorable branch of
the plan's §11.1 trap. Copyright: "(C) 2009 Anthony Blake, (C) 2009-2011 PCB
Contributors." This is portable into tessera's GPL-3.0-or-later.

**Caveat for `docs/PROVENANCE.md`:** this was verified against the gEDA PCB
origin (`russdill/pcb` mirror), *not* against pcb-rnd's current fork
specifically (pcb-rnd's own repo is unreachable and no obvious mirror carries
its current `toporouter.c` path). If a future port draws from pcb-rnd's tree
rather than upstream gEDA PCB, re-verify the header there — forks can and do
relicense or diverge.

---

## Freerouting (`github.com/freerouting/freerouting`) — Java, GPL-3.0

**Algorithm shape**, confirmed from `docs/architecture.md`: `DSN → parser →
board/rule model → autoroute → drc → SES`, with the engine internally called
"maze routing" — shape-based/maze expansion over free-space regions, not a
fixed grid. Matches the plan's "shape-based" label.

**Documented failure modes (`docs/issues/`) — a ready-made test-case
catalog** for tessera's corpus (plan §9.1):

- **Issue 152:** a 2-layer board with a full GND pour produces ~62 clearance
  violations under Freerouting's autorouter — copper-pour/plane awareness is
  effectively absent. Still open.
- **Issue 558:** DSN export doesn't carry board-edge clearance at all, so
  Freerouting routes 0.2mm from the edge when KiCad's rule wants 0.5mm,
  failing KiCad's own DRC after the fact. This is direct, independent
  validation of the plan's central claim (§7.4, §0's flagship criterion) that
  plane-reference/edge-clearance awareness is a real gap Freerouting doesn't
  cover.
- **Issue 383:** no native star-ground support (open enhancement) — matches
  the plan's explicit non-goal acknowledgment (§0) that analog-aware routing
  is out of scope for v1, but worth tracking as a `docs/FUTURE.md` item since
  Freerouting doesn't solve it either.
- `docs/benchmarks.md`: real, non-trivial completion gaps on public benchmark
  boards (e.g. 42-51 of ~100-195 nets left unrouted on several DAC2020 boards
  even with fanout+optimizer enabled) — a concrete completion-rate baseline
  tessera's §0 criterion 2 (≥98% completion) can be measured against.

---

## Rust IPC bindings for `tessera-io-kicad` (plan §8.2)

The plan's assumed repo path for the official binding
(`gitlab.com/kicad/code/kicad-api-rs`) is wrong — it prompted a GitLab auth
wall (dead end). Corrected findings:

| | `kicad-api-rs` (official) | `kicad-ipc-rs` (third-party) |
|---|---|---|
| Actual repo | `gitlab.com/kicad/code/kicad-rs` (crate published as `kicad-api-rs`) | `github.com/Milind220/kicad-ipc-rs` |
| Maintainer | Jon Evans (KiCad core dev, also maintains `kicad-python`) | Solo, 3rd-party |
| Created / last commit | 2024-01-03 / 2025-11-09 | 2026-02-18 / active, v0.5.1 on 2026-05-25 |
| Releases | One ever (0.1.0, 2025-06-07); README calls itself a "development preview" | 13 releases in ~3 months |
| KiCad proto pin | Submodule pinned to tag `9.0.6` — a full major version stale vs. the installed 10.0.3 | Submodule pinned to tag `10.0.1` — only two patch releases behind |
| API surface | 711 LOC total: Track/Arc/Via creation + doc listing only. No net classes, stackup, rule areas, locked state | Generated from the real protos; models `ItemLockState::Locked` and `PcbZoneType::RuleArea` directly (answers plan §8.1 item 4) |
| DRC run/violations | Neither exposes it — both only have `InjectDrcError` (test-marker injection). Confirms this is a KiCad IPC limitation, not a binding gap (matches our own M0 probe finding, `docs/DECISIONS.md` ADR-0002) | Same |
| Known gaps | Effectively unmaintained relative to its own README's admission | Open issue #38: footprint/pad definitions not fully exposed |
| Async/blocking | Blocking (`nng::Socket` directly) | Tokio async-first + blocking wrapper feature |
| License | GPL-3.0-or-later | MIT |
| crates.io reverse-deps | Zero | Zero |

Neither is mature enough to depend on directly — the version-skew problem is
the same failure mode found independently in the Python `kipy` package (see
`docs/DECISIONS.md` ADR-0002), just worse here (a full major version behind,
not just an internal import bug).

**Decision — recorded as ADR-0003 in `docs/DECISIONS.md`:** hand-roll a thin
`prost`-based binding in `tessera-io-kicad` directly against KiCad's own
`.proto` sources (vendor/pin at a tested 10.x tag), the same approach both
existing crates already take internally. This keeps full control over schema
version and upgrade cadence, matches plan §8.2's "never let generated
protobuf types leak past this crate" rule, and avoids inheriting either
crate's staleness. `kicad-ipc-rs`'s MIT-licensed generated Rust
(`src/proto/generated/*.rs`) is a legitimate reference to consult while
writing the codegen setup, since it's already close to current KiCad 10.x —
consulting for structure, not copying, per plan §11.3.

Async vs. blocking is moot for tessera regardless of which binding informs
the design: per plan §2.3's bulk-fetch → route-offline → commit-back
architecture, IPC is called a handful of times per run, never in the routing
hot loop, so async buys nothing here.
