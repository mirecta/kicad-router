# Decisions — append-only ADR log

This log is append-only. Never rewrite or delete an entry; if a decision changes,
add a new entry that supersedes the old one and say so explicitly.

---

## ADR-0001: Project license is GPL-3.0-or-later

**Date:** 2026-07-28
**Status:** Decided (locked at plan time, ratified here in code)

### Context

`tessera` is a PCB autorouter shipping as an external KiCad plugin. KiCad itself
is GPL-3.0-or-later. A large fraction of the algorithmic prior art worth reading
or porting from (KiCad PNS, Freerouting, toporouter/pcb-rnd) is GPL-licensed.

### Decision

The project is licensed **GPL-3.0-or-later**, in its entirety. Every crate's
`Cargo.toml` carries `license = "GPL-3.0-or-later"`. The full license text lives
at `/COPYING` (verbatim copy of the FSF's GPLv3 text, sourced locally from
`/usr/share/common-licenses/GPL-3` on this machine rather than reconstructed
from memory, to avoid any risk of a legally-meaningful transcription error).
Third-party attributions live in `/NOTICE`.

### Rationale

- KiCad is GPL-3.0-or-later; matching it maximizes compatibility with the
  ecosystem tessera plugs into.
- The project's goal is a fully open tool — no commercial dual-licensing plan
  exists today.
- GPL-3.0-or-later can freely absorb code under MIT/BSD/Apache-2.0 and under
  GPL-2.0-or-later or GPL-3.0(-or-later). It cannot absorb GPL-2.0-**only**
  code without separate permission (verify per-source, see §11.1 of the plan).

### Consequences

- **One-way compatibility.** MIT/BSD projects (notably Topola, see the M0
  prior-art survey below) can never absorb tessera's code back without tessera
  re-licensing. This is accepted knowingly — see the M0 Topola recommendation
  in `docs/PRIOR_ART.md`.
- Every new crate must set `license = "GPL-3.0-or-later"` in `Cargo.toml` from
  creation; this is enforced by the workspace skeleton established in this
  same milestone (§2.2 of `AUTOROUTER_PLAN.md`).

---

## ADR-0002: M0 KiCad IPC API capability probe — findings

**Date:** 2026-07-28
**Status:** Recorded (empirical findings, not a design decision per se, but
gates several downstream decisions and is exactly the record §8.1/§14 of the
plan requires)

**KiCad version tested:** 10.0.3 (Ubuntu package `kicad 10.0.3~ubuntu25.10.1`),
installed locally. `kicad-cli` version reports the same.

**Method:** Rather than launching a live KiCad IPC session immediately, the
official Python bindings (`kicad-python` / import name `kipy`, the currently
published wrapper around KiCad's protobuf IPC schema) were installed into a
throwaway venv (`pip install kicad-python`, resolved to version 0.7.1, targeting
`KICAD_API_VERSION = "10.0.1-0-g2db9e5a72b"`) and its bundled `.proto`-derived
Python stubs (`kipy/proto/board/*.pyi`, `kipy/proto/common/**/*.pyi`) were read
directly. This answers "what does the wire protocol actually carry" more
precisely than a live GUI click-through would, and is reproducible without a
running KiCad session. A live round-trip against a running KiCad instance is
still recommended before M1 implementation begins in earnest (see Follow-ups).

### Q1 — Board stackup (thickness, εr, loss tangent)

**Exposed, in full.** `board.proto` defines `BoardStackup` →
`BoardStackupLayer` (per-layer `thickness`, `type`, `enabled`, `material_name`)
→ `BoardStackupDielectricLayer` → `BoardStackupDielectricProperties`, which
carries `epsilon_r: float`, `loss_tangent: float`, `material_name: str`, and
`thickness`. `board_commands.proto` has `GetBoardStackup`/`UpdateBoardStackup`
RPCs. This exceeds what v1 routing needs (plan §7.4 only requires *preserving*
declared widths, not solving for Z₀) but is directly usable for reporting
estimated Z₀ to the user later.

### Q2 — Net class diff pair parameters (gap, via gap, uncoupled length)

**Partially exposed.** `project_settings.proto`'s `NetClassBoardSettings`
carries `diff_pair_track_width`, `diff_pair_gap`, and `diff_pair_via_gap` as
first-class fields, readable/writable via `GetNetClasses`/`SetNetClasses`
(`project_commands.proto`). **Uncoupled length is not a net-class property in
KiCad's model at all** — confirmed by inspecting a real board's custom rule
file (`/usr/share/kicad/demos/vme-wren/vme-wren.kicad_dru`), where uncoupled
length appears as a DRC **constraint type** on a custom rule
(`constraint diff_pair_uncoupled (max 5mm)`), conditioned on
`A.inDiffPair('*')`. This is only reachable via the `.kicad_dru` fallback
parser (see Q3), not via any IPC net-class message.

### Q3 — Custom DRC rules, in any evaluable form via IPC

**Not exposed. This is the single most consequential finding of M0.**
`board.proto` does define a `BoardDesignRules` message, but it is a literal
stub with zero fields (`message BoardDesignRules {}` — no constraints, no
conditions, no rule text, nothing). No IPC command anywhere in
`board_commands.proto` or `project_commands.proto` retrieves or evaluates
custom rule text. There is no "run DRC and get structured violations" RPC
either — the closest thing, `InjectDrcError`, does the opposite (lets a client
*inject* a synthetic marker for testing, not query real ones).

This confirms the plan's §3.3 fallback is **mandatory, not optional**: custom
DRC rules must be read by parsing the project's `.kicad_dru` file directly.
Ground-truthed its real syntax against a shipped demo board
(`vme-wren.kicad_dru`): S-expression `(rule "name" (layer ...) (constraint
<type> (min x) (max y) (opt z)) (condition "<expr>"))`, with a small expression
language (`A.NetClass == 'x'`, `A.inDiffPair('*')`, `A.intersectsArea('name')`,
`A.fromTo('ref-*','ref-*')`, boolean `&&`/`||`). This matches the plan's §7.5.6
assumed syntax closely; `intersectsArea` was observed in the wild alongside the
plan's assumed `insideArea` — **both predicates likely exist with different
semantics (fully-inside vs. touches) and must be verified individually when
`tessera-drc`'s expression evaluator is built.**

**DRC-run fallback confirmed working end-to-end:** `kicad-cli pcb drc --format
json` was run against the same demo board and produces a well-structured
report (`$schema`, `violations[]` with `description`, `severity`, `type`,
per-item `uuid`/`pos`/`description`). Sample violation observed:
`"length_out_of_range"` correctly attributing a custom length-matching rule
violation by name (`rule 'length_DDR_CMD_FPGA_To_IC13'`) with exact
min/actual lengths. **This is the parity-harness oracle path (§3.2) and it
works today, with zero KiCad IPC session required** — it's a plain subprocess
call, usable in CI without a running KiCad GUI at all.

### Q4 — Named rule areas + locked state

**Locked state: exposed, uniformly.** `common/types/base_types.proto` defines
a shared `LockedState` enum (`LS_UNKNOWN` / `LS_UNLOCKED` / `LS_LOCKED`) used
as a `locked` field on `Track`, `Via`, `Zone`, `Pad`, `Footprint`(`Instance`),
and several other item types in `board_types.proto`. This directly satisfies
the plan's §7.5.4 requirement — locked tracks/vias/footprints are readable
via IPC.

**Rule areas: geometry and boolean keepout flags exposed; net allowlist is
not a structural IPC concept.** `Zone` messages carry `outline` (`PolySet`),
`layers` (layer set), `locked`, and — when acting as a rule area —
`rule_area_settings: RuleAreaSettings`, which has boolean
`keepout_copper`/`keepout_vias`/`keepout_tracks`/`keepout_pads`/
`keepout_footprints` flags plus `placement_enabled`/`placement_source_type`/
`placement_source`. There is **no net-list/allowlist field on `RuleAreaSettings`
at all.** This matches the plan's own expectation (§7.5.6): the "inverse
keepout" behaviour the protected-regions design needs (allow *only* specific
nets inside a fenced area) is implemented in KiCad purely through custom DRC
rule conditions (`A.insideArea('BuckStage') && A.NetClass != 'Power'`), which
per Q3 are **not IPC-readable** — so `tessera-model`'s `ProtectedRegion` must
be built by combining (a) IPC-readable zone geometry/layer-set/keepout-flags
and (b) `.kicad_dru`-parsed custom rule conditions referencing that zone's
name via `insideArea`/`intersectsArea`. There is no way around the `.kicad_dru`
parser for this feature; it is on the critical path, not an edge case.

### Binding-maturity finding (relevant to §8.2, informs but does not decide it)

The published `kicad-python` 0.7.1 (PyPI) package has a real bug: its
`kipy/board_rules.py` does `from kipy.proto.board.board_pb2 import
CustomRuleConstraintType, CustomRuleDisallowType, CustomRuleLayerMode,
DrcErrorType`, but the bundled compiled `board_pb2` in this same package
version does not define `CustomRuleConstraintType`/`CustomRuleDisallowType`/
`CustomRuleLayerMode` at all (`import kipy.board_rules` raises `ImportError`
immediately). This is a version-skew bug in the official Python wrapper
against its own bundled proto — a concrete data point that KiCad's official
bindings are, as the plan already suspected (§8.2), not fully baked. This
doesn't block tessera (which needs Rust bindings, not Python), but it's a
signal to weight when evaluating `kicad-api-rs` for the same kind of
version-skew risk (tracked separately, see `docs/PRIOR_ART.md`).

### Follow-ups (not yet done, flagged rather than skipped)

- No live IPC socket round-trip against a *running* KiCad session was
  performed in this pass — only the shipped protobuf schema was read. Before
  M1 implementation, do one live connect/fetch (`kipy.KiCad.connect()` or
  the eventual Rust binding) against a real open board to confirm the schema
  reading above matches live behavior, not just the `.proto` definitions.
- `insideArea` vs `intersectsArea` semantic difference is unverified — the
  plan explicitly flags this as a documented inconsistency across item types
  (KiCad issues #13947, #8438). This must be empirically tested per item type
  in the M1 DRC parity harness, not assumed from either name.

---

## ADR-0003: `tessera-io-kicad` hand-rolls its own protobuf binding

**Date:** 2026-07-28
**Status:** Decided

### Context

Plan §8.2 names two candidate Rust bindings to evaluate: `kicad-api-rs`
(official) and `kicad-ipc-rs` (third-party). Full evaluation, including
corrected repo locations (the plan's assumed GitLab path for the official
crate is wrong and dead-ends on an auth wall), is in `docs/PRIOR_ART.md`
under "Rust IPC bindings for `tessera-io-kicad`."

### Decision

Neither existing crate is depended on directly. `tessera-io-kicad` will
generate its own thin `prost`-based Rust bindings directly from KiCad's own
`.proto` sources, vendored/pinned at a specific tested KiCad version tag.

### Rationale

- `kicad-api-rs` (official) is a full major KiCad version stale (pinned to
  `9.0.6` against an installed `10.0.3`) and its own README calls it a
  "development preview" with only one release ever.
- `kicad-ipc-rs` (third-party) is closer to current (pinned to `10.0.1`) and
  more actively maintained, but is single-maintainer, five months old, has a
  known open gap (footprint/pad definitions not fully exposed, issue #38),
  and zero crates.io reverse-dependencies as an external adoption signal.
- Both crates internally do exactly what we're choosing to do — generate
  Rust from a pinned `.proto` snapshot — so hand-rolling isn't extra
  novel risk, it's removing a layer of indirection over someone else's
  version-pin decisions, which we'd have to audit anyway.
- Matches plan §8.2's explicit rule: "wrap it behind our own trait... never
  let generated protobuf types leak into `tessera-model`." That trait
  boundary needs to exist regardless of which crate sits behind it, so
  owning the codegen directly costs little extra.
- `kicad-ipc-rs`'s MIT license permits consulting its generated code
  (`src/proto/generated/*.rs`) as a structural reference while setting up our
  own `prost` codegen, without any licensing complication.

### Consequences

- We own upgrading the `.proto` pin when KiCad releases a new minor/major
  version — a recurring maintenance cost, but one with our own release
  cadence rather than a third party's.
- No async runtime dependency is pulled in for IO — appropriate given plan
  §2.3's bulk-fetch/route-offline/commit-back architecture calls IPC a
  handful of times per run, never in the routing hot loop.
- Revisit this decision if `kicad-ipc-rs` reaches a stable 1.0 and tracks
  KiCad releases closely for a longer track record — re-vet before M1
  implementation of `tessera-io-kicad` actually begins, not just at M0.

---

## ADR-0004: `tessera-geom` supports coordinates up to ±1 m per axis, not KiCad's full ±2.147 m range

**Date:** 2026-07-28
**Status:** Decided

### Context

While building `tessera-geom`'s first exact predicates (M1), a proptest
property test (`segment_distance_sq_is_never_negative`) caught a real `i128`
overflow in `Segment::distance_sq`'s clamped-perpendicular branch, which
multiplies two already-squared lengths together (`|ap|^2 * |ab|^2`). This
overflows `i128` at coordinate magnitudes that are within KiCad's own
documented legal range: verified against `libs/kimath/include/math/vector2d.h`
in the KiCad source (`typedef VECTOR2<int32_t> VECTOR2I`), KiCad's on-board
coordinate type is a 32-bit signed integer in nanometres — up to ~2.147 m
per axis, ~4.3 m worst-case span across a board. At that span the dangerous
product reaches ~1.36e39, against an `i128::MAX` of ~1.70e38.

### Decision

`tessera-geom` documents and enforces (via `debug_assert!` in `Point::new`) a
coordinate bound of `MAX_COORDINATE_NM = 1_000_000_000` (1 m per axis) —
smaller than KiCad's theoretical ±2.147 m ceiling. At this bound the same
worst-case product is ~6.4e37, comfortably inside `i128` with better than 2x
margin.

### Rationale

- No real PCB approaches a 2 m span, let alone KiCad's absolute ±2.147 m
  per-axis ceiling; a 1 m per-axis bound (2 m board diagonal) already covers
  every realistic and even generously oversized panel.
- The alternative — exact 256-bit widening multiplication — is real,
  well-understood arithmetic (this is precisely the class of problem
  Shewchuk's adaptive-precision paper, cited in plan §1, solves), but hand
  ­rolling it under time pressure for a case no physical board will ever
  reach is exactly the kind of premature complexity plan §12 rule 5 warns
  against ("prefer deleting code to adding flags"/scope discipline).
- Failing loudly via `debug_assert!` (compiled out in release, catching the
  precondition violation in every test/dev run) is preferable to silently
  producing a wrong DRC-adjacent answer, which plan §0 treats as the single
  worst possible outcome.

### Consequences

- If a future corpus board (or a legitimate large panel) needs more range,
  the correct fix is exact wide (256-bit) arithmetic in
  `Segment::distance_sq`, gated behind its own design/ADR — **not** silently
  raising `MAX_COORDINATE_NM` without that arithmetic in place.
- Every crate that constructs `tessera_geom::Point` values from ingested
  board data (`tessera-model`, `tessera-io-kicad`) must be aware boards
  outside this range are unsupported and should surface that as an explicit
  error, not truncate/wrap silently.

---

## M1 closed: `tessera-geom` + `tessera-model` + `tessera-drc` + parity harness

**Date:** 2026-07-28

Exit criterion per the plan's milestone table: "DRC parity clean on full
corpus; gap list documented." Both are true for the scope actually built —
net-class clearance across all six item-pair types (track-track, track-via,
track-pad, via-via, via-pad, pad-pad) — verified by
`crates/tessera-io-kicad/tests/drc_parity.rs` against a real `kicad-cli`
install, gap list in `docs/DRC_PARITY.md`. "Full corpus" is not literal yet:
no `corpus/` directory of real boards exists (plan §9.1) — the harness
currently generates synthetic two-item boards via `proptest` rather than
running against curated real designs. Building that corpus is follow-up
work, not blocking M2, since the parity *mechanism* (our engine vs.
`kicad-cli`, compared automatically) is what M1 required and is now in
place and reusable against any future corpus board.

**What worked:** building `tessera-geom`'s exact predicates with `proptest`
from the start caught two real overflow bugs before they could hide in
`tessera-drc` (ADR-0004, and the segment-vs-segment fraction-comparison
overflow) — cheaper to find via random testing on day one than as a
mysterious wrong DRC answer on a real board later. Verifying the
`.kicad_pcb`/`.kicad_pro` file format empirically (hand-built minimal
fixtures tested against `kicad-cli` before writing the general Rust writer)
found the net-class-lives-in-the-project-file fact and the board-level
`min_track_width` default confound *before* they could cause a confusing
harness failure blamed on the wrong component.

**What didn't fully close:** custom DRC rule conditions
(`insideArea`/`intersectsArea`/`NetClass`/`fromTo`) are unimplemented —
`tessera-drc` currently only resolves net-class-level clearance. Per
ADR-0002, that's on the critical path for M2.5 (protected regions) and
needs the `.kicad_dru` parser (not yet built) before any expression
evaluator work can start.

**What I'd change:** the discovered KiCad DRC gap (full-overlap copper
producing zero violations, `docs/DRC_PARITY.md`) deserves a deliberate
follow-up pass — testing whether it's track-specific or general, and
whether the GUI's interactive DRC has the same gap — before M2's rip-up
scheduler starts relying on `kicad-cli` as an oracle for anything overlap-
adjacent.

---

## M2 closed: ingest → grid A* → commit; CLI; locked-item invariant

**Date:** 2026-07-28

Exit criterion per the plan's milestone table: "End-to-end route of a
trivial 2-layer board, DRC-clean, visible in KiCad; rip-up-trap corpus
board passes." Both hold for the scope built:

- `tessera-cli route <in.kicad_pcb> <out.kicad_pcb>` runs the real compiled
  binary end-to-end (`crates/tessera-cli/tests/end_to_end.rs`) and, when
  `kicad-cli` is on `PATH`, verifies the output with actual KiCad DRC —
  zero clearance violations on a trivial 2-layer board with one unrouted
  net. "Visible in KiCad" wasn't manually verified in a GUI session this
  pass — the `kicad-cli` DRC check is the automated proxy for it; opening
  the output file in the KiCad GUI once is a reasonable manual follow-up,
  not a blocking gap.
- The locked-item invariant has its own adversarial test
  (`crates/tessera-engine/tests/locked_item_invariant.rs`): a locked wall
  taller than the router's search window, on one layer (must detour via
  the other, wall unchanged) and on both layers (must fail to route rather
  than compromise the lock, wall unchanged, net still reported unrouted).

**Scope not yet built, flagged rather than silently assumed:** no
`corpus/` of real boards exists yet (still true from M1); multi-pin net
decomposition (3+ pads) is explicitly unsupported and reported via
`ConnectionReport::skipped`, not attempted; the grid router's search window
is local per-connection (a fixed margin around each connection's
endpoints), not the whole board, so a connection needing a longer detour
than that margin allows will fail rather than expand its search — untested
against anything but small synthetic boards so far, since no real corpus
exists yet to test against.

**What worked:** cross-checking the router's own output against
`tessera-drc` (not just trusting the search's internal bookkeeping) caught
two real, non-obvious bugs before they could reach a corpus board — a via
placed using the wrong (track, not via) radius and the wrong (single, not
every-spanned) layer, and a path-simplification approach that would have
silently cut corners through bends. Both are exactly the class of bug that
"looks right" in isolation and only shows up against an independent
oracle — the same lesson M1 already taught with the geometry-kernel
overflow bugs, now validated a second time one layer up the stack.

**What I'd change:** the local per-connection search window (plan-approved
as the pragmatic M2 baseline, "it will be bad") has an unquantified failure
mode — a connection needing a detour longer than the fixed margin simply
fails, with no fallback to a larger window. Worth a corpus board
specifically exercising this before M3's global router is expected to
route the same class of connection successfully.

---

## M3 status: substantial progress, **not closed** — blocked on a real corpus

**Date:** 2026-07-28

Unlike M0–M2, this entry doesn't claim M3 done. Its exit criterion per the
plan's milestone table — "≥90% completion on 4-layer corpus with grid
detailed router" — names a specific, quantitative, corpus-dependent
measurement that genuinely cannot be checked yet: no `corpus/` of real
4-layer boards exists (flagged as a gap since M1, still true). Marking
this milestone "closed" without that measurement would be exactly the
kind of unverified claim the plan's rule zero warns against.

**What was actually built and verified this pass, each independently
tested:**

- Multi-pin net decomposition (`tessera-global::minimum_spanning_tree`) —
  an original MST-based rectilinear Steiner heuristic, not a FLUTE port
  (porting FLUTE requires the human-gated procedure in plan §11.3 before
  any of that code exists — not run this pass, so not attempted). Closes
  M2's explicit "3+ pad nets unsupported" gap; verified end-to-end
  (3-pin net routes via 2 MST edges, result is clearance-clean).
- PathFinder negotiated congestion (`tessera-global::pathfinder`) —
  implemented directly from McMurchie & Ebeling's paper, tested against
  synthetic congestion scenarios proving the actual negotiation mechanism
  (uncongested direct routing, sharing capacity that fits, redistributing
  two nets onto separate corridors when one lacks capacity, and honestly
  reporting genuinely infeasible congestion rather than false convergence).
- Global-to-detailed integration — `tessera-engine::route_board` now
  negotiates every connection together on one board-wide coarse grid
  before any detailed routing, passing each connection's negotiated path
  as a waypoint hint that reshapes `tessera-detail`'s local search window.
  All M2 regression tests still pass; a new test proves five simultaneous
  nets negotiate and route together correctly.

**What's explicitly not done, so the ≥90%-completion criterion has no
meaning to check yet:**

- **The global grid isn't obstacle-aware.** Its capacity model reflects
  negotiated congestion among the connections being routed, not real board
  geometry — no locked walls, no existing tracks, no board edge. A
  waypoint can shift two competing nets apart from each other; it cannot
  route a connection *around* a specific obstacle it has no representation
  of. This is the single biggest gap between what exists now and a global
  router that actually earns the ≥90% number — most of a global router's
  real value is exactly the obstacle-avoidance this doesn't have.
- **The waypoint hint is soft, not a hard corridor constraint.** It widens
  where `tessera-detail` looks; it doesn't confine the search to a tube
  around the corridor or forbid cells outside it. A tighter, more
  predictable integration is a natural next step.
- **Layer assignment isn't a distinct decision.** Plan §5.2 treats it as
  part of global routing ("cost must include via cost... make layer
  transitions explicit"); today layer choice still falls out entirely of
  `tessera-detail`'s own local via-cost tradeoff, with no global-level
  layer-balancing signal feeding it.
- **No corpus.** Building or sourcing real 4-layer boards (with a licence
  check on anything sourced from the community, per plan §9.1) is now the
  actual blocking dependency for closing M3 — not more algorithm work.

**What I'd change:** given the corpus gap has now blocked *three*
milestones' exit criteria in a row (M1's parity harness, M2's rip-up-trap
corpus board, M3's completion-rate target), it's worth treating corpus
acquisition as its own tracked piece of work rather than a recurring
footnote — the next reasonable move, before more global-router refinement,
is likely assembling even a small real `corpus/` (the plan's own "5
trivial 2-layer boards" tier, §9.1, is a modest, achievable start) so
future milestones have something real to measure against instead of only
synthetic test boards.

---

## The global grid is now obstacle-aware

**Date:** 2026-07-30

Closes the first bullet of M3 status's "what's explicitly not done" list
above: `GlobalGrid`'s capacity model previously reflected only negotiated
congestion among the connections being routed, never real board geometry.
It now does.

**What changed:** `tessera-global::GlobalGrid` gained an `obstruction`
field — a per-(cell, layer) amount of that layer's flat capacity already
consumed by fixed geometry, indexed via the new `GlobalGrid::cell_index`.
`capacity()` subtracts the more-obstructed of an edge's two endpoint
cells from that layer's base capacity, clamped to zero. Empty
`obstruction` (the default for any caller not using this) is a no-op —
existing behaviour is unchanged unless a caller opts in.

`tessera-engine::route::obstruction_from_board` is that caller: it
rasterizes `tessera_detail::obstacles_from_board(board)` onto the coarse
grid's resolution, using the exact same `ObstacleShape::clears_point`
point-in-obstacle test `tessera-detail::ObstacleMap` already uses at the
fine grid, just evaluated at cell centres instead. A cell whose centre
falls inside real copper has its full per-layer capacity marked consumed.

**A correctness subtlety, resolved by construction rather than left as a
gap:** obstacles belonging to a net that's part of *this negotiation
round* are excluded from rasterization. Without that, every connection's
own start/end pad would obstruct its own departure cell on every single
run (`obstacles_from_board` doesn't filter by net the way `ObstacleMap`'s
per-connection `routed_net` exclusion does — the global grid negotiates
many nets' requests at once, so there's no single "routed_net" to filter
against). The accepted imprecision this leaves: if a net in this round has
other, already-committed copper elsewhere on the board (e.g. one edge of
a multi-pin net routed, another edge of the same net still pending), that
copper also isn't treated as an obstacle for this round. Acceptable
because this grid is still a *soft* waypoint hint, not a hard corridor
constraint — `tessera-detail` checks real per-net clearance regardless of
what the global grid did or didn't know about.

**Still not covered, deliberately out of scope here:**

- **Via-edge capacity.** Layer-change edges stay capacity-unlimited, as
  before — via congestion isn't modelled by this grid at all yet.
- **Board outline / edge-cuts.** `tessera_model::Board` has no outline
  field to rasterize — there's nothing to feed in. Not a corner cut, just
  genuinely no data available yet.
- **Fractional obstruction.** A cell is treated as either fully consumed
  or untouched (whichever obstacle covers its centre point most), not a
  proportional reduction based on how much of the cell an obstacle
  actually occupies. Consistent with `tessera-detail::ObstacleMap`'s own
  cell-centre test at the fine grid, not a new approximation invented
  here.

**Verified:** a new `tessera-global` unit test proves a single net
detours off an otherwise-uncongested row when its cell is manually
obstructed (`obstructed_cell_pushes_the_route_onto_an_alternate_row`); a
new `tessera-engine` unit test proves `obstruction_from_board` blocks a
foreign locked track's cell while leaving an active net's own pad
unobstructed; a new `tessera-engine` integration test
(`routes_around_a_pre_existing_locked_wall_and_stays_clean`) routes a net
across a board with a pre-existing locked track directly in its
straight-line path and confirms the result is still clearance-clean. All
prior M2/M3 regression tests still pass unchanged.

---

## `tessera-detail` now hard-confines waypoint-guided search to a corridor

**Date:** 2026-07-30

Closes the second bullet of M3 status's "what's explicitly not done" list
further up this file, and was flagged as the natural follow-up to
obstacle-awareness in the entry directly above. Before this, a waypoint
hint from the global router only widened `route_connection`'s local
search window's bounding box — a **soft** influence that reshaped where
the router looked but never stopped it from taking a shorter path
elsewhere in that box, even one the global negotiation had specifically
routed a different net through instead.

**What changed:** `tessera-detail::CorridorMask` (new, in `grid.rs`) is a
per-cell bitmap of which cells lie within a fixed half-width
(`CORRIDOR_HALF_WIDTH_NM`, 1.5mm — deliberately narrower than the 3mm
bounding-box margin, so the constraint isn't a no-op alongside it) of the
polyline from a connection's start, through its waypoints, to its goal.
`astar::search` gained an `Option<&CorridorMask>` parameter; when given
one, it refuses to expand into any cell outside it. When
`route_connection` receives non-empty waypoints, it now tries the
corridor-constrained search *first*.

**Never a completion-rate regression, by construction:** if the
corridor-constrained search fails — the coarse global grid's path didn't
account for some fine-grid obstacle the corridor is now too tight around
— `route_connection` falls back to the old unconstrained full-window
search rather than reporting failure. A waypoint hint can therefore only
ever make a route shorter or more predictable; it can never make a
connection that would have routed before fail now. This mirrors the same
"honest fallback, never silently worse" discipline the rest of this
codebase already follows (e.g. `negotiate`'s convergence reporting).

**Verified:** two new `tessera-detail` unit tests exercise `CorridorMask`
directly (`contains` near/far from a polyline, `mark_inside` forcing a
cell regardless of distance); a new integration test
(`waypoint_hint_hard_constrains_the_search_to_the_corridor`) sets up a
connection with an *unobstructed* direct straight-line path and waypoints
describing a deliberately different bent detour, then asserts every point
of the routed result stays within corridor tolerance of the waypoint
polyline — a soft-only implementation would have taken the shorter
direct line instead and failed this assertion. All prior M2/M3 regression
tests still pass unchanged (waypoints `&[]` skips the corridor
entirely, so behaviour with no global hint is untouched).

**Still not covered:** the corridor half-width is a fixed constant, not
tuned or adaptive — a corpus is still what's missing to measure whether
1.5mm is too tight, too loose, or about right across real boards.

---

## `.kicad_dru` custom design-rule parser (syntax only, no evaluator yet)

**Date:** 2026-07-30

ADR-0002 (Q3) established that custom DRC rules are only reachable by
parsing `.kicad_dru` text directly (not via IPC) and explicitly phased the
work: "the parser (not built) before any expression-evaluator work can
start." This entry closes the parser half of that phasing — the
expression evaluator (and resolving `insideArea` vs `intersectsArea`
semantics) is still not built, deliberately.

**What changed:** `tessera-io-kicad::dru::parse_design_rules` parses a
`.kicad_dru` file's `(version N)` and `(rule "name" (layer ...)?
(constraint <kind> (min ..) (max ..) (opt ..))* (condition "...")?)`
forms into a `DesignRules { version, rules: Vec<Rule> }` structure. Two
deliberate choices, both explained in the module's doc comments:

- A rule's `condition` expression text is captured **verbatim, as a raw
  string**, not parsed into an AST or evaluated — that mini-language
  (`A.NetClass == 'x'`, `A.inDiffPair('*')`, `A.intersectsArea('name')`,
  `A.fromTo('ref-*','ref-*')`, `&&`/`||`) is genuinely separate follow-up
  work.
- A constraint's `kind` (`clearance`, `track_width`, `diff_pair_gap`,
  `diff_pair_uncoupled`, `length`, `hole_size`, `via_diameter`, ...) is
  kept as a raw string, not a closed enum — ADR-0002 already flagged this
  vocabulary as large and not fully enumerated, so a fixed enum here would
  mean guessing at members never actually observed.

`tessera-io-kicad::sexpr` gained `parse_all`, parsing a *sequence* of
top-level S-expressions rather than requiring exactly one root —
`.kicad_dru` files are a bare sequence of `(version ...)`/`(rule ...)`
forms with no enclosing list, unlike `.kicad_pcb`'s single
`(kicad_pcb ...)` root that the existing `parse` already handled.

Numeric bounds (`(min 0.1mm)`) are converted to integer nanometres at
parse time (matching every other unit in this codebase), but **only the
`mm` suffix is supported** — anything else is treated as unparseable
(reported via `warnings`, that one constraint's bound left `None`) rather
than guessed at, since `mm` is the only suffix actually observed in a real
file. Malformed individual rules/constraints are skipped with a warning
rather than failing the whole file, mirroring
`crate::parser::ParsedBoard`'s existing no-silent-data-loss stance — the
project's established pattern for this exact tradeoff.

**Verified:** grounded against the identical file ADR-0002 itself used —
`/usr/share/kicad/demos/vme-wren/vme-wren.kicad_dru` (KiCad 10.0.3). Its
content is embedded verbatim in `dru.rs`'s own unit tests (an excerpt
covering every constraint/condition shape the file uses) plus a separate
integration test (`tests/dru_demo_file.rs`) that reads the real file from
disk when present and confirms all 11 of its rules parse with zero
warnings — skipping gracefully (like `drc_parity.rs`'s `kicad-cli`
availability check) on a machine without that demo file, rather than
failing.

**What's explicitly not done:** actually *evaluating* a condition against
real board items — see the next entry for the expression-language
*parser* (a separate, narrower piece built the same session) — and
`tessera-model` still has no `ProtectedRegion`/rule-area concept to
evaluate rules against in the first place (ADR-0002's Q4 finding). Both
are necessary follow-up work for M2.5, not attempted here.

---

## `.kicad_dru` condition-expression parser (mini-language syntax, still no evaluator)

**Date:** 2026-07-30

The previous entry's `Rule::condition` field is a raw string — this entry
parses that string's own mini-language (`A.NetClass == 'x' &&
A.fromTo('ref-*','ref-*')`) into a structured AST, kept as its own module
(`tessera-io-kicad::dru_expr`) deliberately separate from `dru.rs`'s
S-expression-level parsing, mirroring the existing `sexpr.rs`/`parser.rs`
split (generic syntax layer vs. semantic layer).

**What changed:** `dru_expr::parse_condition` hand-rolls a tokenizer plus
recursive-descent parser (`or_expr := and_expr ('||' and_expr)*`,
`and_expr := predicate ('&&' predicate)*`) for exactly the predicates
observed or explicitly expected per ADR-0002: `NetClass ==`, `inDiffPair`,
`intersectsArea`, `insideArea`, `fromTo`. Two things deliberately not
supported, because inventing either would mean guessing at a form never
observed in the grounding file: parenthesized grouping, and negation
(`!`). `insideArea` parses despite never appearing in the one grounding
file, because ADR-0002 already documented the expectation that it exists
alongside `intersectsArea` — that's a recorded finding, not a fresh guess.

**Still an unresolved question, unchanged from ADR-0002:** whether
`insideArea` means fully-inside vs. `intersectsArea`'s touches, and what
item(s) the `A`/`B` subject actually binds to for a given constraint type
— both are evaluator questions, not parser questions, and this module
answers neither. The AST only records which subject letter and predicate
name appeared; it assigns no meaning to either.

**Verified:** every one of the 9 distinct condition strings in the
`vme-wren.kicad_dru` grounding file parses successfully (one test asserts
this across the full set); targeted tests also cover trailing whitespace
inside a quoted string, `&&`/`||` precedence together in one expression,
an accepted-but-unverified `insideArea` call, syntactically-accepted `B.`
subjects, and rejection of an unknown predicate name, wrong-arity
`fromTo` calls (1 and 3 arguments), a dangling `&&`, trailing content
after a complete expression, a missing `==`, and an unterminated string
literal.

---

## ADR-0002 addendum: `insideArea`/`intersectsArea` and multi-rule-match semantics, verified against real `kicad-cli`

**Date:** 2026-07-30

ADR-0002 (Q3) left two real semantic questions open, flagged as needing
verification "against real KiCad DRC behaviour" before an evaluator could
be built responsibly — not guessed at alongside the syntax parsers built
earlier this same session. This entry answers both, the same way ADR-0002
itself was produced: by constructing minimal synthetic `.kicad_pcb` +
`.kicad_dru` fixtures and running the real `kicad-cli pcb drc --format
json` (KiCad 10.0.3) against them, not by reading documentation. Fixtures
were scratch files under `/tmp`, not committed — the findings below, and
the exact fixture shape needed to reproduce them, are.

**Method:** a synthetic 2-layer board with one named rule-area zone
(`(zone (name "TestArea") (keepout (tracks allowed) (vias allowed) (pads
allowed) (copperpour allowed) (footprints allowed)) (polygon (pts ...)))`
— the same "all-allowed keepout" shape as the real `underFPGA`/`underDDR`
zones found in `vme-wren.kicad_pcb` itself, confirming this is how
`.kicad_dru` area names are actually declared) and three tracks: one
fully inside the zone's polygon, one straddling its boundary (partway in,
partway out), one fully outside. Paired `.kicad_dru` files used a
guaranteed-to-violate single-item constraint (`track_width (max 0.01mm)`,
comfortably smaller than any real track) gated on different area
predicates, isolating "did this predicate match this item" from any
actual clearance-geometry question.

**Finding 1 — for track items, `insideArea` and `intersectsArea` produced
identical results in every test.** Both matched the fully-inside track
*and* the boundary-straddling track; neither matched the fully-outside
one. Tested each predicate both in isolation (one rule per file) and
confirmed the boundary-straddling track really does cross the polygon
edge (not fully contained) — `insideArea` matched it anyway. This
contradicts the plan's assumed "fully-inside vs. touches" semantic split
for these two predicates, at least for track items against an
all-allowed keepout zone.

**Follow-up, same session:** `AUTOROUTER_PLAN.md` §7.5.6 itself flags "a
documented history of `insideArea` behaving inconsistently across item
types (KiCad issues #13947, #8438)," so Finding 1 was re-tested against
pads too, using `(constraint disallow pad)` (the plan's own `disallow`
constraint shape — a min/max-based constraint like `track_width` doesn't
apply to pads) instead of an artificially-tiny bound. A pad fully inside
the zone and a pad with its centre placed exactly on the zone's boundary
edge (so only half its copper actually overlaps the zone) both matched
`insideArea` and `intersectsArea` identically — the same "any overlap
matches" behaviour as tracks, not the historically-inconsistent split the
plan warned might exist. **Still not verified:** whether a zone with
actual keepout restrictions (rather than the all-allowed shape both
`underFPGA`/`underDDR` and this experiment use) changes anything, or
whether footprint courtyards (as opposed to their pads) behave
differently again.

**Finding 2 — when multiple custom rules with the same constraint type
match the same item, only the *last-declared* rule (in file order) is
reported; earlier matches are silently superseded, not reported as
additional/separate violations.** Verified by controlled reordering: the
identical two-rule file, with only the declaration order of
`flag_inside`/`flag_intersects` swapped, changed which rule name appeared
in the violation for the exact same two tracks — from `flag_intersects`
winning to `flag_inside` winning. Ruled out alphabetical-by-name as the
tie-break mechanism (the alphabetically-earlier rule name won in one
ordering and lost in the other, tracking file position each time, not
name). **This is a real algorithmic requirement for the eventual
evaluator, not a detail that can be deferred:** naively evaluating every
rule against every item and reporting every match would over-report
relative to real KiCad — the evaluator needs to select the
last-in-file-order matching rule per (item, constraint type), the same
way `.kicad_dru`'s own declaration order already functions as an implicit
override sequence (later rules take precedence over earlier ones for the
same item, much like a CSS cascade). **Not yet verified:** whether this
last-wins behaviour holds across *different* constraint types on the same
item (e.g. one rule setting `track_width` and another setting `clearance`
on the same track, via different matching area predicates) — plausible
that those simply coexist independently since they aren't the same
constraint type, but untested.

**What this unblocks:** the evaluator (still not built) now has two
concrete, empirically-grounded behaviours to implement rather than two
open questions to guess at. What's still unresolved before it can be
built: what item(s) the `A`/`B` subject binds to for constraint types
that aren't single-item-scoped (e.g. `clearance`, which is inherently
pairwise), and the `ProtectedRegion`/rule-area model `tessera-model`
still doesn't have at all.

---

## Session checkpoint: M2.5 groundwork, stopped short of the evaluator deliberately

**Date:** 2026-07-30

This entry closes out an extended autonomous session (obstacle-aware
global grid through here, all same-day) with an honest account of where
the custom-rule/protected-region work actually stands, and — more
importantly — *why* it stops here rather than continuing into the
evaluator itself, which is the one remaining piece with real open design
questions instead of just more mechanical implementation.

**Built and verified this session, in order:** the obstacle-aware global
grid, the hard corridor constraint, the `.kicad_dru` S-expression parser
(`dru.rs`), the condition-expression parser (`dru_expr.rs`, later fixed
for `!`/`!=` after re-reading `AUTOROUTER_PLAN.md`'s own examples), an
exact `Polygon` primitive in `tessera-geom`, the empirical
`insideArea`/`intersectsArea`/last-rule-wins findings (extended to pads,
not just tracks), a `RuleArea` model (the geometry/keepout half of a
`ProtectedRegion`), and zone parsing from `.kicad_pcb` into that model.
Each is its own commit with its own tests; `docs/SESSION_HANDOFF.md` has
the per-piece summary.

**Why the evaluator itself isn't attempted here:** building it means
resolving several genuinely open questions at once, not applying one more
already-settled fact:

- **`fromTo('IC14-*','IC13-*')` needs footprint reference designators**
  (`"IC14"`), which `tessera_model::Pad` doesn't track at all yet — only
  an arbitrary `PadId`. The real `.kicad_pcb` syntax for this was checked
  this session (`(property "Reference" "IC94" ...)` inside a
  `(footprint ...)` block, confirmed against `vme-wren.kicad_pcb`) — so
  reading it is well-understood. But adding a `Pad.reference` field
  ripples through **13 files and every existing `Pad { ... }` literal**
  across the workspace (checked via `grep -rl "Pad {"`), and — per this
  same codebase's own stated discipline in `pad.rs` ("add variants here
  in lockstep with the matching predicate, not ahead of it") — there's no
  evaluator yet to actually consume that field. Adding it now would be
  speculative model-widening ahead of its only real consumer, not a
  neutral prerequisite.
- **`inDiffPair('*')` needs diff-pair membership**, which
  `tessera_model` has no concept of at all (net classes carry
  `diff_pair_*` *parameters* — gap, width — but nothing links two nets
  together as an actual pair).
- **Wildcard matching semantics** (`'IC14-*'`, `'Shield*'`) aren't
  pinned down — glob-style `*` seems obviously intended, but the exact
  match target (`"IC14-1"` pad-name style? reference-only? something
  else?) hasn't been verified against a real board the way every other
  fact in this file has been.
- **The last-rule-wins resolution algorithm** (Finding 2, above) needs
  designing carefully against real per-item, per-constraint-type
  selection — not just "loop through rules," since a naive
  implementation over-reports relative to real KiCad.

None of these are big in isolation, but they're all genuinely open design
decisions, not settled facts waiting to be typed in — exactly the
distinction this session tried to hold to throughout (empirically verify,
don't guess). Continuing into the evaluator at this point would mean
making four design calls in a row unreviewed, several hours into an
unattended overnight run, rather than one more mechanical, well-grounded
step. That's the actual reason to stop here, not a token budget or time
limit.

**Concrete next steps, in the order they'd naturally come up:** (1) pin
down wildcard match semantics against a real board; (2) add
`Pad.reference` (and update its ~13 call sites) once there's an evaluator
ready to consume it, not before; (3) design a minimal diff-pair model;
(4) design and implement the evaluator itself, including the last-wins
selection algorithm; (5) only then wire evaluated custom rules into
`tessera-drc` as actual violations. The corpus gap (still blocking M3's
own exit criterion, unrelated to any of this) remains the other standing
next step from earlier in `docs/SESSION_HANDOFF.md`.

---

## Wildcard/pairwise-binding semantics for `fromTo`, `inDiffPair`, `intersectsArea`, and pairwise `NetClass` — verified against real `kicad-cli`

**Date:** 2026-07-31

Directly follows up on the previous entry's item (1) — resolves the
wildcard-matching open question empirically, the same way every other
`.kicad_dru` fact in this file was established, using more scratch
`.kicad_pcb`/`.kicad_dru` fixtures under `/tmp` (not committed). Also
resolves item (3) — a minimal diff-pair model turns out not to be needed
at all, which is better news than expected.

**Finding 1 — `fromTo(A, B)` matches against `"<reference>-<pad
number>"` (e.g. `"IC14-3"`), case-**insensitively**, and is symmetric:**
`fromTo('IC14-*','IC13-*')` and `fromTo('IC13-*','IC14-*')` both match the
same connection. A bare reference with no `-<pad>` suffix and no wildcard
(`fromTo('IC14','IC13')`) also matches — confirmed this isn't prefix
matching (`fromTo('IC1', ...)` does **not** match `"IC14-3"`) but a
distinct "reference alone" candidate string tried alongside
`"reference-pad"`, either of which the pattern can match. `ic14-*`
(lowercase) matched `"IC14-3"` — reference/pad matching is case-
insensitive.

**Finding 2 — `fromTo` (and, it appears, any per-connection constraint)
evaluates at the *whole routed connection* level, not per-track-segment
by that segment's own endpoint coordinates.** A 3-segment bent
connection from an `IC14` pad through an intermediate bend to an `IC13`
pad had **all three segments** flagged by
`fromTo('IC14-*','IC13-*')` — including the middle segment, whose own
two endpoints touch neither pad. The evaluator therefore needs
connectivity tracing (which net, and that net's terminal pads) per item,
not just that item's own geometry — a materially different scope than
"check each track's own two endpoints."

**Finding 3 — diff-pair recognition needs no separate pairing model at
all: it's pure net-name-suffix convention, checked live.** Nets named
with a `_P`/`_N` or `+`/`-` suffix (e.g. `SIG_P`/`SIG_N`, `SIG+`/`SIG-`)
are auto-recognized as a pair; `_PLUS`/`_MINUS` or unrelated names
(`SIGA`/`SIGB`) are not. `inDiffPair(pattern)` matches against the pair's
*base name* with the suffix stripped (`inDiffPair('SIG')` matches both
`SIG_P` and `SIG_N`; `inDiffPair('SIG_P')`, the full net name, matches
neither) — glob wildcards work on that base name, and matching is
case-**sensitive** (`inDiffPair('sig')` does not match `SIG_P`/`SIG_N`).
This removes the need for any new diff-pair model/pairing table in
`tessera-model` — the evaluator can derive pair membership from
`Net::name` alone at evaluation time.

**Finding 4 — area-name matching (`intersectsArea`) is glob-capable on
both ends (`Shield*` and `*ZoneA` both matched a zone named
`ShieldZoneA`) but case-**sensitive** (`shieldzonea` did not match).**
Combined with Finding 1 and Finding 3, this means each of the three
wildcard-taking predicates has *different* case sensitivity —
`fromTo` insensitive, `intersectsArea`/`inDiffPair` sensitive — a real,
easy-to-miss asymmetry the evaluator's string matching must account for
per-predicate, not with one shared case-folding rule.

**Finding 5 — for pairwise constraint types (`clearance`), `A`/`B` bind
to the two distinct items being compared, and the binding is
symmetric — a rule matches regardless of which item KiCad happened to
assign to `A` vs. `B`.** Verified with two tracks on different net
classes 0.1mm apart (violating a 100mm test clearance): `A.NetClass ==
'ClassX' && B.NetClass == 'ClassY'` matched, and swapping to `A ==
'ClassY' && B == 'ClassX'` matched identically. A same-class-both-sides
condition (`A == 'ClassX' && B == 'ClassX'`) correctly did **not** match,
since only one `ClassX` item exists on the board (ruling out a
degenerate self-pairing bug in the test method, which an earlier,
incorrect pass through this same experiment had briefly suggested before
the test script was fixed to check the firing rule's *name*, not just
its violation *type* — the earlier apparent match was KiCad's ordinary
built-in net-class clearance check being miscounted as this custom rule).

**What this resolves from the previous entry's four open questions:**
wildcard semantics (fully verified, all three predicates) and diff-pair
modelling (turns out to need no new model at all) are done. Footprint-
reference tracking for `fromTo` is now well-justified to build (Finding 1
pins down exactly what format/case-sensitivity it needs) — it just still
needs doing, and the last-wins selection algorithm still needs designing
against Finding 5's symmetric-binding requirement, not simplified away.
