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

---

## `Pad.reference`/`Pad.number`, and the `.kicad_dru` custom-rule evaluator itself

**Date:** 2026-07-31

Closes out the four-item checklist from the "Session checkpoint" entry —
all four are now done, in this order: `Pad` gained `reference` (owning
footprint's designator) and `number` (the pad's own KiCad-native
designator), both read from real `.kicad_pcb` syntax
(`tessera_io_kicad::parser`'s `footprint_reference` looks up the
`(property "Reference" ...)` block *by name*, not by assuming it's
declared first among a footprint's several same-tagged `property`
entries). This was deliberately deferred in the checkpoint entry until
`fromTo`'s exact match format was known — the "Wildcard/pairwise-binding
semantics" entry settled that, so this is no longer speculative
model-widening. Touched all 12 files with an existing `Pad { ... }`
literal.

`tessera_io_kicad::dru_eval` is the evaluator: `eval(Expr, ItemFacts) ->
bool` and `resolve_constraint` (the last-declared-rule-wins selection).
Every behaviour it implements is one of this session's empirical
findings, not a fresh assumption — per-predicate case sensitivity,
`fromTo`'s symmetric candidate matching, `inDiffPair`'s suffix-derived
base-name matching with no persistent pairing model needed. It's a pure
function of a new `ItemFacts` struct, deliberately **not** wired to a
real `tessera_model::Board` yet — populating `ItemFacts` for a real item
needs net-connectivity tracing (to find a connection's two terminal pads
for `fromTo`) and precomputed rule-area intersection per item, both
genuine remaining work, kept separate so the evaluator's own logic stays
testable against synthetic facts without needing a real board first.

**What's left before this is wired into an actual DRC violation:**

1. Build `ItemFacts` from a real `Board` — connectivity tracing
   (probably extending `tessera_model::connection`'s existing
   unrouted-connection-finding logic to also report *already-routed*
   connections' terminal pads) and per-item rule-area membership
   (`RuleArea::outline.intersects_segment`, already available, just not
   called from anywhere real yet).
2. Wire `resolve_constraint`'s resolved values into `tessera-drc` as
   actual violations (a resolved `track_width`/`clearance`/etc. becomes a
   check the same way net-class clearance already is).
3. Still unverified: `A`/`B` binding for constraint types this session
   didn't specifically test (only `clearance` was checked for pairwise
   symmetric binding), and whether `disallow`'s bare item-type args
   change binding/evaluation in any way distinct from bound constraints.

**Verified:** 13 new unit tests in `dru_eval` covering every predicate,
the glob matcher (prefix/suffix/multi-wildcard/case-sensitivity), `Not`/
`And`/`Or` combination, and `resolve_constraint`'s last-wins selection
plus its unconditional (`condition: None`) case. Full workspace `fmt`/
`clippy --all-targets -D warnings`/`test --workspace` clean throughout
(`tessera-io-kicad`'s lib is up to 59 tests, from 19 at the start of this
session).

---

## First real `.kicad_dru` violation: `tessera-drc::check_track_width`

**Date:** 2026-07-31

Closes item (2) of the previous entry's remaining checklist for
`track_width`: a custom `.kicad_dru` rule can now produce an actual
`tessera-drc` violation against a real `Board`, not just against
synthetic `ItemFacts`.

**What changed:** `tessera-drc` gained a `tessera-io-kicad` dependency
(a regular dependency; `tessera-io-kicad`'s existing dependency on
`tessera-drc` is dev-only, for its own DRC-parity test harness, so this
doesn't create an actual build cycle) and
`custom_rules::check_track_width(board, design_rules) ->
Vec<CustomRuleViolation>`. For each track, it builds real `ItemFacts` —
net class and name from `Board::net_class_for`/`Board::nets`, diff-pair
partner via a live net-name-suffix lookup across `Board::nets` (not a
stored pairing — matches the "Wildcard/pairwise-binding semantics"
entry's finding that none is needed), rule-area membership via
`RuleArea::outline.intersects_segment` (existed since the `RuleArea`
entry, just not called from anywhere real until now), and — for 2-pin
nets only — `fromTo`'s two terminal pads via a new
`Board::two_pin_net_endpoints`. `dru_eval::resolve_constraint` picks the
applicable `track_width` constraint (if any); a track whose actual width
falls outside its resolved bound becomes a `CustomRuleViolation`.

**Why `Board::two_pin_net_endpoints` is scoped to exactly 2 pads:** a
3+-pad net's Steiner-decomposed routing edges (`tessera_global::
minimum_spanning_tree`'s output, consumed by `tessera_engine::
route_board`) are never stored back onto `Board` — only the resulting
`Track`/`Via` items are, with no record of which original edge each came
from. So "which 2 of this net's N pads does this specific track
actually connect" genuinely isn't recoverable from `Board` alone today;
returning `None` for anything but exactly 2 pads is an honest boundary,
not an arbitrary restriction to be lifted casually later — lifting it
means either preserving decomposition metadata through routing, or
computing real copper connectivity from geometry, either of which is its
own project.

**Deliberately not attempted here — real remaining work, not corners
cut:**

- **`length`** needs summing every track segment belonging to a
  connection, not one item's own geometry — a different aggregation
  shape than this module's per-item loop.
- **`clearance`** is inherently pairwise (`A`/`B` bind to two distinct
  items, per the previous entry's Finding 5) — needs its own
  item-*pair*-scoped `ItemFacts` construction, not a per-item loop.
- **`disallow`** has no numeric bound to compare against at all — its
  violation is just "this rule's condition matched," a different
  `CustomRuleViolation` shape than "measured value vs. resolved bound."
- Only `track_width` on `Track` items is wired — `hole_size`,
  `via_diameter`, `diff_pair_gap`, `diff_pair_uncoupled`, and any other
  bound-shaped kind would need their own item-type/measurement mapping,
  structurally similar to `check_track_width` but not automated here.

**Verified:** 7 new `tessera-drc` unit tests — a track narrower than a
resolved minimum is flagged with the right rule name/bound/values, a
track within bounds isn't flagged, a non-matching condition correctly
gates out a track, `fromTo` correctly matches (and correctly fails to
match) via real terminal-pad lookup, `intersectsArea` correctly uses the
track's own geometry against a real `RuleArea`, and `inDiffPair` finds a
real companion net by live suffix lookup. Full workspace `fmt`/
`clippy --all-targets -D warnings`/`test --workspace` clean.

---

## `disallow` wired in — and its last-wins resolution needed its own function

**Date:** 2026-07-31

Extends the previous entry's `check_track_width` to the plan's other
canonical constraint shape: `(constraint disallow track via)`. This one
surfaced a genuine, empirically-confirmed wrinkle that `resolve_constraint`
alone would have gotten wrong.

**What changed:** `tessera-drc::check_disallow(board, design_rules) ->
Vec<DisallowViolation>` checks every track and via. Tracks reuse the
`track_width` path's `item_facts_for_net` (renamed from
`track_item_facts` now that it's shared) and exact
`intersects_segment`-based area membership; vias use a centre-point-only
area-membership approximation instead (documented as a known
simplification — a via's own diameter isn't accounted for near a
boundary it merely sits close to).

**Why `resolve_disallow` had to be a distinct function from
`resolve_constraint`, not `resolve_constraint(..., "disallow")` plus an
`args` check on the result:** built two synthetic-but-kicad-cli-verified
scenarios (both in `/tmp`, not committed) to check whether `disallow`'s
last-wins competition is scoped by constraint kind alone (like
`track_width`'s numeric-bound competition) or by which item types a
rule's `args` actually lists:

- A later `(constraint disallow via)` rule, matching the same condition
  as an earlier `(constraint disallow track)` rule, does **not** suppress
  the track rule for a track item — real `kicad-cli` still cites
  `disallow_track`. Both are `disallow`-kind rules with identical
  matching conditions, so a kind-only-scoped last-wins (what
  `resolve_constraint` does) would have incorrectly picked the later
  `via`-only rule and silently cleared the track's real violation.
- Two rules that both genuinely list `track` in their `args`, both
  matching, *do* resolve to only the last-declared one — confirming
  last-wins competition is real here too, just scoped more narrowly (by
  item type actually listed, not by kind alone).

`dru_eval::resolve_disallow` implements the correctly-scoped version;
`resolve_constraint` is untouched and remains correct for bound-shaped
kinds like `track_width`, where this distinction doesn't arise (there's
only one bound per constraint, not a list of items it may or may not
apply to).

**Verified:** 4 new `dru_eval` unit tests reproducing both `kicad-cli`
scenarios directly (not suppressed by a different-item-type rule;
last-wins when genuinely competing; no match when no rule lists the
item type; no match when the condition itself doesn't match), plus 4 new
`tessera-drc` tests covering tracks, vias (via centre-point area
membership), and the same real not-suppressed scenario end-to-end
through `check_disallow`. Full workspace `fmt`/
`clippy --all-targets -D warnings`/`test --workspace` clean throughout.

**Still scoped to `track_width` and `disallow` only** — `length`,
`clearance`, and other bound-shaped kinds (`hole_size`, `via_diameter`,
`diff_pair_*`) remain, each needing its own measurement/binding shape per
the previous entry's reasoning.

**A specific note on `length`, since it looked like the natural next
piece and turned out not to be a quick addition:** started implementing
it (sum a 2-pin net's track segment lengths, compare against a resolved
`length` constraint) and stopped before finishing, because it surfaces
two genuinely unverified assumptions rather than just more of the same
pattern:

1. **Does `length`'s condition evaluate per-segment or over the whole
   connection?** Finding 2 (the "Wildcard/pairwise-binding semantics"
   entry) verified this specifically for `fromTo` against a
   `track_width` constraint — every segment of a multi-segment
   connection was flagged, including one touching neither terminal pad.
   Whether the *same* whole-connection scoping applies when the
   constraint kind being checked is `length` itself (as opposed to some
   other kind alongside a `fromTo` condition) hasn't been tested.
2. **What does area membership (`intersectsArea`) even mean for an
   aggregate, connection-spanning check?** A single item's geometry has
   one unambiguous membership test; a connection's total length doesn't
   obviously have one — union across every segment, intersection, or
   something else entirely is a real design choice, not a fact to look
   up, and this session's kind of empirical grounding (construct a board,
   run real `kicad-cli`, observe) hasn't been done for this specific
   question.

Also unresolved: `length` violations are inherently about a whole
connection/net, not a single item — `CustomRuleViolation`'s `item:
ItemRef` shape (track/via/pad only) doesn't fit, the same way
`disallow` needed its own `DisallowViolation` type rather than reusing
`CustomRuleViolation`. Verifying (1) and (2) against real `kicad-cli`
first, then designing the right violation shape, is the honest next
step — not guessing at either to keep moving.

---

## `length` constraint semantics, verified — then wired into `tessera-drc`

**Date:** 2026-08-16 (session resumed on a different machine — see
`docs/SESSION_HANDOFF.md`; environment re-verified clean before starting:
`cargo fmt`/`clippy --all-targets -D warnings`/`test --workspace` all
passed, `kicad-cli` 10.0.3 present)

Picks up exactly where the previous session's "`disallow` wired in..."
entry stopped: the two `length`-specific questions it flagged as
unverified, checked against real `kicad-cli` before writing any more code.

### Question 1 — does `length`'s measurement aggregate the whole connection, or evaluate per-segment?

Built a controlled fixture (not the real vme-wren board this time —
minimal, so nothing else could confound the result): a 2-pin net `SIG`
with **two** 1mm track segments end-to-end (0→1mm→2mm) between two pads,
under `(constraint length (max 1.5mm)) (condition "A.fromTo('*-1','*-1')")`.

**Result: aggregates.** `kicad-cli` reports exactly **one** violation,
`"...max length 1,5000 mm; actual 2,0000 mm"` — the sum of both 1mm
segments, not either segment's own 1mm length (which would individually
pass). The violation's `items` are the **two terminal pads**, not any
track segment — confirming `length` is a connection-level measurement,
citing the connection's endpoints, not a per-item one. This matches (and
now directly confirms with a controlled fixture, not just an incidental
observation) the very first M0 probe finding in this log's ADR-0002 §Q3,
which showed a `length` violation on the real vme-wren board reporting an
aggregate figure close to the rule's bound rather than one segment's own
length.

### Question 2 — what does `intersectsArea` mean for a connection-spanning check?

Three fixtures, same 2-segment connection, rule now
`A.fromTo('*-1','*-1') && A.intersectsArea('Zone1')`:

1. A **passive** (all-`allowed`, no actual keepout — see the pitfall
   below) named zone covering **only the first segment** (x: -0.5mm to
   0.5mm): rule does **not** fire.
2. The same zone widened to cover **both segments** (x: -0.5mm to 2.5mm):
   rule fires (`actual = 2.0mm`, same as question 1).
3. To rule out a boundary-touch artefact: a zone covering **only the
   second segment**, but with a **clean 0.1mm gap** from the first
   segment's endpoint (x: 1.1mm to 2.5mm, vs. the segment boundary at
   1.0mm): rule does **not** fire. (A version of this zone starting
   exactly at x=1.0mm, touching the first segment's endpoint at a single
   point, *did* fire — which is what motivated fixture 3: that result was
   ambiguous between "any real overlap counts" and "a degenerate
   boundary-touch counts," and only mattered to resolve because it would
   otherwise have looked like an "any segment" result.)

**Result: requires every segment, not any segment** — the opposite of
`track_width`'s already-documented per-item semantics, where each track
is evaluated independently against the same condition and only the
matching ones are checked. For a `length` (whole-connection) constraint,
`intersectsArea`/`insideArea` needs the *entire* connection's geometry
inside/touching the area, not just part of it.

**A pitfall hit while building fixture 1, worth recording:** a `.kicad_pcb`
`zone` with `(keepout (tracks not_allowed) ...)` is a real, enforced
keepout — it produced `items_not_allowed` violations for the very track
and pad it was supposed to be a passive named-area marker for, completely
masking the `length` rule I was trying to isolate. Fixed by setting every
keepout flag to `allowed`, making the zone purely a named-area marker with
no enforcement of its own — worth remembering for any future fixture that
needs a rule area without also gating real geometry.

### What changed

- `tessera_geom::Segment::length_nm(self) -> i64` — a new, explicitly
  reporting-only method (goes through `f64::sqrt`, unlike every other
  method on `Segment`; see its own doc comment for why that's fine given
  `MAX_COORDINATE_NM`'s bound, same reasoning as the `nm()` conversions
  already used elsewhere in this workspace).
- `tessera-drc::custom_rules::check_length(board, design_rules) ->
  Vec<LengthViolation>`: for every net with `Board::two_pin_net_endpoints`
  and at least one routed track, sums every belonging track segment's
  `length_nm()`, resolves the applicable `length` constraint via the
  existing (unmodified) `dru_eval::resolve_constraint` — no `disallow`-style
  distinct resolution function was needed here, since `length` is a plain
  bound-shaped kind like `track_width`, not an args-scoped one like
  `disallow` — and reports a `LengthViolation` (a new type, not
  `CustomRuleViolation`, since a length violation's "item" is a whole
  connection/net, not a single track/via/pad — the same reasoning that
  gave `disallow` its own `DisallowViolation` type).
- `areas_intersecting_all` builds `ItemFacts.areas` as the **intersection**
  (not `check_track_width`'s union-per-single-item) of every segment's own
  intersecting areas, directly encoding question 2's finding — no change
  needed to `dru_eval` itself, since `intersectsArea`/`insideArea` already
  just check membership in whatever `areas` slice they're given; the
  aggregation semantics live entirely in how the caller builds that slice.
- Explicitly skips 3+-pin nets (not just because `two_pin_net_endpoints`
  returns `None` there, but because — unlike `track_width`/`disallow`,
  which check one item's own property regardless of net topology —
  summing "every track segment on this net" for a multi-pin net wouldn't
  correspond to any single `fromTo`-style connection KiCad's `length`
  constraint actually measures; it would just be the net's total routed
  copper, a different and less meaningful question).

**Verified:** 5 new `tessera-drc` unit tests reproducing all three
`kicad-cli` fixtures above directly (aggregation, within-bound is not
flagged, both `intersectsArea` boundary cases), plus a net with no routed
segments and a 3-pad net both correctly producing no violations. 2 new
`tessera-geom` unit tests for `length_nm` (3-4-5 triangle exactness;
degenerate zero-length segment). Full workspace `fmt`/
`clippy --all-targets -D warnings`/`test --workspace` clean throughout —
172 tests total (counted via `cargo test --workspace`'s own summary
lines, not estimated), up from 165 before this entry's 7 new tests.

**Still not done — the same list as before, minus `length`:**
`clearance` (inherently pairwise), `hole_size`, `via_diameter`,
`diff_pair_*`. Multi-pin (3+-pad) net support for any per-connection check
(`length` or a future `fromTo`-conditioned one) remains genuinely
unsupported, not approximated — see `Board::two_pin_net_endpoints`'s docs.

## `clearance` constraint semantics, verified — the first genuinely pairwise `.kicad_dru` kind

**Date:** 2026-08-16, same session as the `length` entry above. Continues
"rad za radom, vsetko" (one by one, everything, don't skip anything):
`clearance` is next on the gap list after `length`.

Every constraint kind implemented so far (`track_width`, `disallow`,
`length`) evaluates its condition against a *single* item (or single
connection). `clearance` is different: it names two items (`A` and `B` in
KiCad's own condition syntax), and its bound is measured between them.
Before writing any evaluator code, four questions were checked against
real `kicad-cli` using minimal hand-built fixtures (two tracks on
different net classes, spaced so the gap fails a candidate custom bound
but passes the net-class default, or vice versa):

1. **Does a rule written `A.NetClass == 'X' && B.NetClass == 'Y'` match
   the physical pair regardless of which item `kicad-cli` happens to bind
   to `A` vs `B`?** Yes — confirmed by writing the condition both ways
   (`A.NetClass == 'X' && B.NetClass == 'Y'` and the reversed
   `A.NetClass == 'Y' && B.NetClass == 'X'`) against the same physical
   track pair; both fire identically. This matches the wildcard/pairwise-
   binding semantics already established for `fromTo`/`inDiffPair` in the
   earlier "Wildcard/pairwise-binding semantics..." entry, now confirmed
   to extend to `clearance`'s two-sided condition too.
2. **Does a single-subject condition (only mentioning `A`, not `B`) match
   if *either* item qualifies, or only if the specific item KiCad calls
   `A` qualifies?** Either item — a condition like `A.NetClass == 'HV'`
   fires for a pair regardless of which of the two items is actually on
   the `HV` net class.
3. **Does a matching custom rule's bound *replace* the net-class default,
   or combine with it (e.g. `max`)?** Replace, outright — a custom rule
   with a small bound (`50_000nm`) matching a pair whose net-class default
   is huge (`5mm`) applies the small bound; the net-class default is not
   consulted at all once a custom rule matches. Same "replace, don't
   combine" behavior already established for `track_width`.
4. **When two custom `clearance` rules both match the same pair, which
   wins?** Simple last-wins, like `track_width` — not `disallow`'s
   item-type-scoped variant (there's no item-type axis to scope by here;
   a pair is a pair).

### What changed

- `tessera_io_kicad::dru_eval::eval_pairwise(expr, facts_x, facts_y) ->
  bool`: evaluates a condition against an unordered item pair by trying
  both bindings (`A` = `facts_x`/`B` = `facts_y`, then swapped) and
  OR-ing the results — directly encoding finding 1. Built on a new
  private `eval_with_binding(expr, facts_a, facts_b)` that routes each
  `Expr::Predicate`'s evaluation to `facts_b` only when its `subject` is
  literally `"B"`, `facts_a` otherwise (so finding 2 — a single-subject
  condition matching on either side — falls out for free: whichever
  binding of the pair puts the qualifying item in `A`'s slot will match).
  The existing `eval(expr, facts)` now delegates to
  `eval_with_binding(expr, facts, facts)` — a behavior-preserving refactor
  verified by running all 17 pre-existing `dru_eval`-module tests
  unchanged before adding any new ones.
- `dru_eval::resolve_constraint_pairwise(rules, facts_x, facts_y, kind) ->
  Option<(&Rule, &Constraint)>` mirrors `resolve_constraint`'s simple
  last-wins scan (finding 4) but matches conditions via
  `eval_pairwise` instead of `eval`.
- `tessera-drc::clearance`'s small geometry/layer-sharing helpers
  (`pad_circle`, `via_circle`, `track_via_share_layer`,
  `via_via_share_layer`, `track_pad_share_layer`, `via_pad_share_layer`,
  `pad_pad_share_layer`) were changed from private `fn` to `pub(crate)` so
  `custom_rules.rs` could reuse them exactly rather than duplicating
  ~150 lines of near-identical geometry logic across two files.
  `via_layer_range` stays private — only used inside `clearance.rs`
  itself.
- `tessera-drc::custom_rules::CustomClearanceViolation` (a new type,
  distinct from `CustomRuleViolation` — it names two items, `a` and `b`,
  not one) and `check_custom_clearance(board, design_rules) ->
  Vec<CustomClearanceViolation>`, which mirrors `clearance.rs::
  check_clearance`'s six-pair-type structure (track/track, track/via,
  track/pad, via/via, via/pad, pad/pad) but resolves each pair's required
  distance via a new `resolved_pairwise_clearance_nm` helper: try
  `dru_eval::resolve_constraint_pairwise` first (finding 3 — a matching
  custom rule's bound replaces the net-class default outright), falling
  back to `Board::resolved_clearance_nm` only when no custom rule matches
  (finding 3's converse).

**Verified:** 7 new `tessera-io-kicad` unit tests for `eval_pairwise`/
`resolve_constraint_pairwise` (symmetric matching regardless of item
order, single-subject-matches-if-either-item-qualifies, no match when
neither qualifies, `eval`'s behavior-preservation under the refactor,
last-wins, no-match-returns-none). 5 new `tessera-drc` unit tests for
`check_custom_clearance` reproducing all four `kicad-cli` findings above
directly (symmetric order-independence, replace-not-combine, fallback to
net-class default when no custom rule matches, last-wins for competing
rules), plus a same-net-never-violates regression (the pre-existing
`resolved_clearance_nm`/`check_clearance` invariant, confirmed to still
hold through the new pairwise-resolution path). Full workspace `fmt`/
`clippy --all-targets -D warnings`/`test --workspace` clean throughout —
185 tests total (via `cargo test --workspace`'s own summary lines,
recounted from a `git stash` of the committed state, not estimated), up
from 173 before this entry's 12 new tests (7 in `tessera-io-kicad`, 5 in
`tessera-drc`).

**Still not done:** `hole_size`, `via_diameter`, `diff_pair_*`. Multi-pin
(3+-pad) net support for any per-connection check remains genuinely
unsupported — see `Board::two_pin_net_endpoints`'s docs.

## `hole_size`/`via_diameter` constraints, verified — and `Pad` gains a real drill field

**Date:** 2026-08-16, same session as the `clearance` entry above.
Continues "rad za radom, vsetko" (one by one, everything): next on the
gap list after `clearance`.

Both are single-item bound-shaped kinds, the same shape as `track_width`
— no new evaluator machinery needed, unlike `clearance`. What needed
checking against real `kicad-cli` first was *which item types* each one
applies to, using the same two-fixture pattern as before (a via with an
undersized diameter/drill; a through-hole pad with an undersized drill):

1. **`via_diameter`** only ever fires for vias — a via's own copper
   annulus diameter (`Via::diameter_nm`, already modelled). No pad or
   track ever appears as this constraint's violated item.
2. **`hole_size`** fires for **both** vias (`Via::drill_nm`, already
   modelled) *and* through-hole pads — reported as a `drill_out_of_range`
   violation naming the PTH pad directly (`"PTH pad 1 [NETA] of J1"`).
   This was the real finding: `tessera_model::Pad` had no drilled-hole
   field at all before this entry (it only ever modelled a pad's copper
   shape), so a pad-scoped `hole_size` check was structurally impossible
   without a model change, not just an unwired one.

### What changed

- `tessera_model::Pad` gains `drill_nm: Option<i64>` — `Some(d)` for a
  through-hole pad's circular drill diameter, `None` for a surface-mount
  pad (no hole). Circular drills only, matching this crate's pre-existing
  "only `Circle` pad shape modelled" scope — KiCad's oval/slotted drills
  aren't represented. This is a breaking field addition to a public
  struct-literal type; every `Pad { .. }` construction site across the
  workspace (12 files, mostly test fixtures) needed a `drill_nm: ...`
  field added — mechanical, no behavior change to any of them
  (`drill_nm: None` everywhere except the two new drill-specific tests).
- `tessera_io_kicad::parser::parse_one_pad` now reads an optional
  `(drill D)` child (the same single-number circular form already used
  for vias) into the new field. Pad type (`smd`/`thru_hole`) itself is
  still not distinguished/stored — `drill_nm.is_some()` *is* the
  thru-hole signal, so a separate `pad_type` field would be redundant for
  every use this workspace has today.
- `tessera-drc::custom_rules::check_via_diameter` and `check_hole_size`
  (the latter checking both vias and through-hole pads, skipping
  `drill_nm == None` pads) — both follow `check_track_width`'s exact
  shape: `dru_eval::resolve_constraint` for last-wins resolution,
  `bound_violated` for the min/max check, reporting a `CustomRuleViolation`
  (no new violation type needed, unlike `clearance`/`length`, since these
  are single-item bounds just like `track_width`).

**Verified:** 5 new `tessera-drc` unit tests (via diameter in/out of
bounds; via hole size, PTH pad hole size, and SMD-pad-is-never-checked for
`hole_size`). 2 new `tessera-io-kicad` parser unit tests (a through-hole
pad's `(drill 0.3)` reads as `300_000`nm; a plain `smd` pad's `drill_nm`
is `None`). Full workspace `fmt`/`clippy --all-targets -D warnings`/
`test --workspace` clean throughout — 192 tests total (via `cargo test
--workspace`'s own summary lines), up from 185 before this entry's 7 new
tests (5 in `tessera-drc`, 2 in `tessera-io-kicad`).

**Deliberately not attempted this entry: `diff_pair_gap`/
`diff_pair_uncoupled`.** Both are genuinely harder than every kind
implemented so far, `clearance` included: they need real *coupled-segment*
geometry — matching which portions of two diff-paired tracks are actually
routed in parallel, measuring the local gap between them along that
coupled run, and (for `uncoupled`) summing the run length where they
*aren't* parallel-routed together. `tessera-geom` has no
parallelism/projection/interval-overlap primitives for this yet, and nets
`inDiffPair`-recognized as a pair (partner-net lookup already exists, see
`diff_pair_partner_name`) says nothing about whether their tracks were
actually routed as a physical pair. This is a materially bigger design
problem than a single sitting's worth of "verify against kicad-cli, wire
it up" work the way every other kind in this gap list has been — it needs
its own geometry design, not a rushed fit into `custom_rules.rs`'s
existing single-item/pairwise shapes. Left as its own explicit follow-up
rather than silently dropped from the gap list.

## Multi-pin `fromTo`/`length` support, verified — and a real per-segment `fromTo` finding split off for later

**Date:** 2026-08-16, same session as the two entries above ("rad za
radom, vsetko" continues: this is task list item #21, the next unresolved
`.kicad_dru` gap after `hole_size`/`via_diameter`). `Board::
two_pin_net_endpoints` has always deliberately returned `None` for a
3+-pad net rather than guess (see that method's own docs) — this entry is
the empirical work needed to replace "deliberately doesn't guess" with a
real answer, at least for `length`.

### The experiment

A 3-pad daisy-chained net (`J1`—2mm track—`J2`—3mm track—`J3`, one
net, no branching) with three competing `.kicad_dru` rules, one per
`fromTo` pairing (`fromTo('J1','J2')`, `fromTo('J1','J3')`,
`fromTo('J2','J3')`), each a `length (max 1mm)` bound (deliberately tiny
— any real routed length trips it):

- **Only one rule fires — the one declared *last* in the file** —
  confirmed by reordering the three rules three different ways (moving
  each to the last position in turn) and observing the *matching* rule
  change each time, with the *reported total* staying identical
  (`5.0mm`, both segments summed) regardless of which one won. This is
  exactly `dru_eval::resolve_constraint`'s existing last-wins mechanism,
  unmodified — the only thing that needed explaining was *why all three
  conditions seemed to be true simultaneously* for the resolver to have
  anything to pick between.
- **The resolution: `fromTo(a, b)` on a whole-net check matches if *any
  two distinct* pads on the net satisfy `a`/`b`, not just directly-wired
  ones.** `J1` and `J3` have no track segment directly between them (the
  route goes through `J2`), yet `fromTo('J1','J3')` matched just as
  readily as `fromTo('J2','J3')` — connectivity through intermediate
  copper is irrelevant to `fromTo`'s own semantics; it's asking "are both
  of these pads part of this connection at all," not "is there a direct
  wire between them."
- **`actual` is still the sum of every track segment on the net** — the
  already-established `length` semantics (see the `length`-constraint
  entry above) hold unchanged for 3+-pin nets; nothing net-topology-
  specific about the measurement itself, only about which `fromTo` rule
  gets to apply it.

A **separate experiment on the same fixture, using `track_width` instead
of `length`** (four competing rules, one per `fromTo` ordering including
both directions) showed the opposite: **`track_width`'s `fromTo` is
evaluated per *segment*, using facts that differ between the two
segments of the same net** — the `J1`→`J2` segment matched
`fromTo('J1','J2')`/`fromTo('J2','J1')` but *not* `fromTo('J2','J3')`;
the `J2`→`J3` segment matched the reverse. A third experiment (a *2-pin*
net routed with a bend — one segment touching only the origin pad plus a
bare bend vertex, the other touching only the destination pad plus that
same vertex) showed **both** segments matching `fromTo(origin,
destination)` even though neither segment's own two ends directly touch
both pads. Reconciling the two: a segment's real `fromTo` identity is
found by walking outward from each of its own two ends through connected
copper, treating an ordinary bend vertex (no pad, degree 2) as
transparent but **stopping at the first real pad encountered** — a pad
never "sees past" itself to another pad on the far side of it, matching
why the `J1`—`J2` segment resolved to `(J1, J2)` and not `(J1, J3)`.

This is a real, well-defined, useful finding — but implementing it means
a genuine copper-connectivity graph walk (following track/via chains,
stopping at pads, handling branches at junctions with 3+ items touching
one point) that doesn't exist anywhere in this codebase yet. It's a
materially bigger and riskier change than the `length` fix below (which
only needed a different *set* of already-available facts, not new
connectivity-walking code), and it touches `check_track_width`/
`check_disallow`'s well-tested existing 2-pin behavior, where a wrong
edge case would be a real regression, not just a missed opportunity. Not
implemented this entry — split off as its own task so it isn't lost, not
silently dropped.

### What changed

- `tessera_io_kicad::dru_eval::ItemFacts.connection_endpoints` changed
  from `Option<[EndpointFacts<'a>; 2]>` to `Vec<EndpointFacts<'a>>` — an
  owned, variable-length list (empty = no known endpoints, same meaning
  as the old `None`). An owned `Vec` sidesteps the lifetime problem a
  borrowed slice would have caused (the caller-computed endpoint list
  needs to be constructible by different functions with different
  policies, not just one fixed-shape array built inline).
- `dru_eval::from_to_matches_any_pair` replaces the old fixed-two-slot
  `Some([e0, e1]) => ... || ...` match: for *every* ordered pair of
  **distinct** indices into the endpoint list, checks whether `a` matches
  one and `b` matches the other. Requiring distinct indices (not just
  "any endpoint matches `a`, any endpoint matches `b`, possibly the same
  one") is what makes this an exact generalization of the old 2-element
  behavior rather than a looser, subtly-wrong one — a regression test
  (`from_to_requires_two_distinct_endpoints_not_the_same_one_twice`)
  pins down the difference: `fromTo('IC14','IC14')` must **not**
  self-match against a board with only one pad actually named `IC14`.
- `tessera-drc::custom_rules` gains `all_pad_endpoints_for(board, net)`
  (every pad on the net, any pin count) alongside the existing
  `connection_endpoints_for` (2-pin only), and a new
  `item_facts_for_net_all_pads` alongside `item_facts_for_net` — used
  **only** by `check_length`, which is the sole check whose `fromTo`
  semantics are actually whole-net-scoped. `check_track_width`/
  `check_disallow`/`check_via_diameter`/`check_hole_size` are completely
  unchanged — still 2-pin-only for `fromTo`, since the per-segment
  connectivity-walk they'd need is the follow-up work described above,
  not this entry's fix.
- `check_length`'s `board.two_pin_net_endpoints(net).is_none() { continue
  }` gate is gone — the function now checks every net with at least one
  routed segment, any pin count.

**Verified:** 2 new `tessera-io-kicad` `dru_eval` unit tests
(multi-endpoint `fromTo` matching any distinct pair; the
self-match-must-not-happen regression). 2 new `tessera-drc` tests
replacing the old (now-incorrect) `length_skips_multi_pin_nets`: a plain
`NetClass`-conditioned rule now correctly checks a 3-pin net's whole
routed length (`2.0mm`, both segments summed), and `fromTo('IC1',
'IC3')` matches on a 3-pin net despite no direct track between those two
pads. Full workspace `fmt`/`clippy --all-targets -D warnings`/`test
--workspace` clean throughout — 195 tests total (via `cargo test
--workspace`'s own summary lines), up from 192 before this entry's net 3
new tests (2 in `tessera-io-kicad`, net +1 in `tessera-drc` after
replacing one test with two).

**Split off as its own task:** a real per-segment/per-via copper-
connectivity graph walk (stop-at-first-pad, as described above) so
`check_track_width`/`check_disallow`'s `fromTo` also works on 3+-pin
nets — and, as a side effect, becomes more correct for bent
(multi-segment) 2-pin routes too, which the current 2-pin-net-wide
lookup only gets right by coincidence (there being exactly 2 pads on the
net at all, regardless of which segment is being checked).

## Board outline (`Edge.Cuts`) parsed and fed into the global grid's capacity model

**Date:** 2026-08-16, same session as the three entries above (task list
item #22, next after multi-pin `fromTo`/`length`). This was the real
`Board::outline` gap `negotiate_global_routes`'s docs had flagged since
the obstacle-aware-grid work: "board outline/edge-cuts (no such field
exists on `Board` yet)."

### Why this needed real parsing work, not just a field

A minimal rectangular test fixture could use a single `gr_rect`, but a
real board's `Edge.Cuts` outline is a loop of individually-declared
`gr_line`/`gr_arc` items with no guaranteed order (checked directly
against this session's own corpus board, `royalblue54l-nfc-antenna`:
its outline mixes straight edges and rounded corners, arcs included).
Two real design questions followed from that:

1. **Arcs.** This crate models no arc geometry anywhere (tracks, rule
   areas, now outlines) — approximating one into a polyline would be
   the first place this project ever traded exactness for convenience,
   directly against the plan's "do not silently polygonise" stance
   (plan §4.1, already cited for track arcs). So: any `Edge.Cuts` item
   that's a `gr_arc` makes the whole outline unbuildable — `None` plus
   an explicit warning, not a silent drop and not an approximation.
   Confirmed this is a real, common case, not a rare edge case: the
   corpus board itself has arced corners and now produces exactly one
   such warning.
2. **Chaining unordered segments into a loop.** `gr_line` items aren't
   declared in walk order or a consistent direction in real files. The
   assembly algorithm (`chain_into_closed_polygon`) starts from an
   arbitrary segment and repeatedly finds *any* remaining segment
   sharing an endpoint with the current chain tail (checking both of
   its own endpoints, since direction isn't consistent either),
   consuming it — checked directly with a declaration-order-shuffled,
   direction-mixed fixture reproducing exactly this. If segments don't
   close into exactly one loop using every single segment (a gap, a
   branch, or more than one disjoint loop), that's `None` plus a
   warning too, not a best-effort partial polygon.

### What changed

- `tessera_model::Board.outline: Option<Polygon>` — `None` means either
  "no `Edge.Cuts` graphics at all" (not a parse failure) or "found some
  but couldn't build a single closed polygon" (a parse failure, always
  paired with a warning explaining which of the two reasons above).
- `tessera_io_kicad::parser::parse_board_outline` (plus
  `chain_into_closed_polygon`, `rect_edges`, `parse_two_point_item`):
  reads root-level (not footprint-local — `Sexpr::find_all` is
  direct-children-only, so a footprint's own courtyard/silkscreen
  graphics are never mistaken for the board outline) `gr_line`/`gr_rect`
  items tagged `(layer "Edge.Cuts")`, bails per the two rules above, and
  otherwise returns the assembled `Polygon`.
- `tessera-engine::route::apply_outline_obstruction`: when
  `board.outline` is known, every global-grid cell (on every layer)
  whose centre falls outside it gets its full per-layer capacity marked
  consumed — the same mechanism `obstruction_from_board` already uses
  for real copper obstacles, just triggered by "outside the boundary"
  instead of "inside an obstacle's shape." **This only strengthens the
  global router's soft waypoint guidance — it is not a hard
  outline-enforcement guarantee.** A connection whose global path is
  blocked entirely by the outline still falls back to
  `tessera-detail`'s unconstrained local search (per
  `negotiate_global_routes`'s pre-existing fallback, unchanged), which
  has no idea the outline exists — see that function's updated docs.
  Real hard enforcement (rejecting/steering a detailed route that
  actually crosses `Edge.Cuts`) is a distinct, not-yet-scoped gap in
  `tessera-detail`'s own obstacle model, not silently claimed as solved
  here.

**Verified:** 6 new `tessera-io-kicad` parser unit tests (a rectangular
outline from four `gr_line`s; the same outline with edges declared out of
order and in mixed directions; a single `gr_rect` shortcut; an arc on
`Edge.Cuts` produces `None` + a warning; an open (non-closing) outline
produces `None` + a warning; no `Edge.Cuts` graphics at all produces
`None` with *no* warning). 2 new `tessera-engine` unit tests directly on
`obstruction_from_board` (a cell outside a synthetic square outline is
fully blocked on every layer, a cell inside is untouched; a board with no
known outline obstructs nothing extra, unchanged from before this
entry). The existing corpus-board regression test
(`royalblue54l_nfc_antenna_is_clearance_clean`) needed its hardcoded
expected-warning-count updated from 2 to 3 (the new, correct Edge.Cuts-
arc warning) and gained an explicit assertion that this board's outline
is `None` — its own outline has arcs, so this is the expected outcome,
not a regression. Full workspace `fmt`/`clippy --all-targets -D
warnings`/`test --workspace` clean throughout — 203 tests total (via
`cargo test --workspace`'s own summary lines), up from 195 before this
entry's 8 new tests (6 in `tessera-io-kicad`, 2 in `tessera-engine`).

**Still not covered:** via-edge capacity in the global grid (pre-existing
gap, unchanged); hard outline enforcement at the detail-routing level
(see above); multi-loop outlines (cutouts, disjoint board pieces) and
arc-containing outlines are both `None` rather than approximated, per the
reasoning above — upgrading either is real follow-up geometry work, not
a quick fix.

## Corridor/constant tuning: investigated, genuinely blocked — not a vague "need more boards" excuse

**Date:** 2026-08-16, same session as the four entries above (task list
item #23, next after board outline). Before writing anything up as
"tuned," checked whether the one corpus board this project has
(`royalblue54l-nfc-antenna`) could support the experiment at all — it
can't, and the reason is concrete, not just "one board isn't a corpus."

`CORRIDOR_HALF_WIDTH_NM` (`tessera-detail::router`, currently a fixed
1.5mm) exists to balance two things against each other: constraining a
waypoint-guided search enough to shrink the search space and shorten
paths, without constraining it so much that real congestion from
*other, competing nets* forces a detour the corridor can't accommodate.
Measuring whether 1.5mm is actually a good value needs a board with
real multi-net congestion — several nets whose direct routes cross the
same region — so a route can fail or succeed differently at different
corridor widths.

Checked what the one corpus board actually offers, directly (not
assumed from its name): it parses to **zero pads and zero routable
connections**. Both of the file's two real pads are non-circular FPC
connector pads — outside this crate's "circular pads only" parsing
scope (module docs, unchanged since M2) — so `tessera-io-kicad` skips
both entirely (the corpus regression test's two pad-shape warnings,
already asserted). With no pads, `Board::find_unrouted_connections`
finds nothing to route, so there's no way to even strip this board's
existing routing and re-run `tessera-engine::route_board` against it —
let alone compare completion rate or path quality across different
corridor widths. And even if the pads did parse, this board has exactly
**one** net (`/ANT`) — a spiral antenna trace — so it could never
exercise multi-net congestion regardless.

This isn't a "corpus is thin, do your best" situation — it's zero
usable data points for this specific question, confirmed by actually
running the numbers rather than assumed. Task #23 is genuinely blocked
on task #18 (corpus is still 1 of the plan's modest first-tier 5
boards, and that one board turns out to be unusable for this
particular question even so) — not a shortfall this session can
responsibly paper over with a plausible-sounding number pulled from
inadequate data. That would be exactly the kind of false-rigor mistake
this project's whole empirical-verification discipline exists to
prevent, just aimed at a constant instead of a `.kicad_dru` semantic.

**Not resolved. Left `pending`, not marked done, with this reasoning
attached** so a future session (or one with a broader corpus available)
doesn't have to re-derive why the obvious "just try it on the corpus
board" approach doesn't work here.

## `diff_pair_gap`/`diff_pair_uncoupled` constraints, verified and implemented

**Date:** 2026-08-16, same session as everything above (task list item
#24, next after the corridor-tuning investigation). This was explicitly
deferred earlier this session (the `hole_size`/`via_diameter` entry) as
needing "real coupled-segment geometry... a materially bigger lift" —
revisited now that the earlier tasks are done, rather than left
permanently on the shelf.

### The condition-shape surprise

The very first fixture (two parallel tracks, 0.2mm wide, 0.3mm centre-
to-centre — an obvious `diff_pair_gap` violation against a `(min
0.4mm)` bound) produced **zero violations** with the condition written
the same pairwise way `clearance` needs: `"A.inDiffPair('SIG') &&
B.inDiffPair('SIG')"`. Before concluding the constraint just doesn't
work, checked whether `inDiffPair` even matches this fixture at all
(via a `track_width` rule reusing the same condition) — it does, for
both tracks. The real fix, found by testing a single-subject condition
instead (`"A.inDiffPair('SIG')"` — no `B` at all, the same shape
`track_width`/`length` already use): **`diff_pair_gap` (and
`diff_pair_uncoupled`) are single-subject constraints, not pairwise
ones**, despite comparing two tracks — kicad-cli evidently resolves
"which two tracks make a pair" via its own diff-pair recognition
(net-name suffix matching, the same mechanism `inDiffPair` already
uses), not via an explicit `A`/`B` binding the rule author writes. With
that fix, `diff_pair_gap` fired correctly: `actual 0.1000mm` — exactly
the 0.3mm centre distance minus both tracks' 0.1mm half-widths,
confirming the measurement is edge-to-edge copper, not centreline.

A follow-up fixture (`SIG_P` spanning 5mm, `SIG_N` spanning only the
first 3mm of it) confirmed `diff_pair_uncoupled`'s measurement: `actual
2.0mm` — exactly `SIG_P`'s excess length beyond where `SIG_N` ends, not
either track's own full length or their sum. Only **one** violation was
reported for the pair in both fixtures, not one per track.

### What changed

- `tessera_geom::Segment::diff_pair_coupling(self, other) ->
  Option<DiffPairCoupling>` — new geometry, not a generalization of an
  existing predicate. `None` if the segments aren't exactly parallel
  (exact integer cross-product test, no float in *that* decision) or
  either is degenerate. Otherwise computes, via projection onto the
  shared direction: `gap_nm` (centreline perpendicular distance),
  `coupled_length_nm` (the projected-overlap run both segments share),
  and `uncoupled_length_nm` (both segments' own length outside that
  overlap, summed). **Reporting-only, like `Segment::length_nm`** — goes
  through `f64::sqrt`/division throughout, the same accepted exception
  to this crate's exact-integer-predicates rule, for the same reason
  (a real value to compare against a bound, not a predicate decision).
- `tessera-drc::custom_rules::coupled_diff_pair_tracks(board)` resolves
  which track pairs this module treats as coupled at all: two nets that
  are each other's diff-pair partner, each with **exactly one** routed
  track segment, same layer, geometrically parallel. Deliberately
  narrow — a bent or multi-segment diff-paired route has no coupling
  computed here, the same honest-gap stance as `length`'s multi-pin
  boundary before this session generalized it, not a guess at which
  segments "really" pair up.
- `check_diff_pair_gap`/`check_diff_pair_uncoupled` — both single-item-
  shaped resolution (`dru_eval::resolve_constraint`, ordinary last-wins,
  matching the condition-shape finding above), reporting a new
  `DiffPairViolation` (names both tracks, since the finding *is* about
  a pair, even though resolution itself is single-subject).
  `check_diff_pair_gap` subtracts both tracks' half-widths from the
  coupling's centreline `gap_nm` before comparing against the resolved
  bound — the same net-width-aware split `clearance.rs::
  check_track_track` already uses for ordinary clearance.

**Verified:** 6 new `tessera-geom` unit tests for `diff_pair_coupling`
(non-parallel and degenerate inputs both `None`; the two real `kicad-
cli` fixtures above reproduced exactly, including the 0.1mm gap and
2.0mm uncoupled length; direction-reversed segments give the same
result; fully non-overlapping projections give zero coupled length and
the full sum as uncoupled). 6 new `tessera-drc` unit tests (gap
violation with the correct pair-not-per-track reporting; within-bounds
gap; uncoupled-length violation; within-bounds uncoupled; a net with two
segments is skipped entirely, not guessed at; tracks on different
layers are skipped). Full workspace `fmt`/`clippy --all-targets -D
warnings`/`test --workspace` clean throughout — 215 tests total (via
`cargo test --workspace`'s own summary lines), up from 203 before this
entry's 12 new tests (6 in `tessera-geom`, 6 in `tessera-drc`).

**Still not covered:** multi-segment (bent) diff-paired routes — a real,
common case on actual boards, deliberately not approximated here (see
`coupled_diff_pair_tracks`'s docs). Extending to that needs the same
"walk connected segments, find the true coupled run" kind of
connectivity work already identified (and deferred, task #25) for
`track_width`/`disallow`'s per-segment `fromTo` — likely worth doing
together once either is tackled, not two separate connectivity walks.

## `track_width`/`disallow`'s `fromTo` now walks real per-segment connectivity

**Date:** 2026-08-16, same session as everything above (task list item
#25, the last item on the original gap list). Implements the mechanism
the "Multi-pin `fromTo`/`length` support" entry already found and
verified against real `kicad-cli` but deliberately didn't wire in at
the time, citing regression risk to `check_track_width`/
`check_disallow`'s well-tested existing behavior.

### Why this was safe to do after all

The earlier entry's two fixtures pinned down the exact algorithm: walk
outward from each of a track's own two ends through connected copper —
transparently through a bare bend vertex (another track touching that
point with no pad there), but *stopping* at the first real pad
encountered, never seeing past it. Re-reading `Board::
two_pin_net_endpoints`'s existing 2-pin behavior against that algorithm:
for a single straight segment directly between two pads (every existing
test's fixture shape), the walk from each end hits a real pad
immediately, on the first step — the *exact same answer*
`two_pin_net_endpoints`-based lookup already gave. So this wasn't
"replace working code with new, riskier code" — it was "replace a
net-wide shortcut with the real per-segment computation it happens to
agree with in every case the existing tests cover." Ran the full
pre-existing `tessera-drc` test suite (40 tests) *before* writing a
single new test, confirming zero regressions, before trusting the
change at all.

### What changed

- `tessera-drc::custom_rules::item_facts_for_track(board, track, areas)`
  — a new facts-builder used only by `check_track_width`/
  `check_disallow`'s **track** handling (via handling in `check_disallow`
  is unchanged, still `item_facts_for_net`-based — a via has no single
  "own segment" to walk from the way a track does, and via `fromTo`
  behavior was never empirically verified this session, so it wasn't
  touched). Builds `connection_endpoints` from
  [`walk_to_pad_endpoint`] on each of the track's own two ends, instead
  of requiring the whole net to have exactly 2 pads.
- `walk_to_pad_endpoint(board, net, point, visited)`: recursive walk
  from `point` through same-net tracks, stopping and returning a pad's
  facts the moment one is found at the current point; `None` on a dead
  end, an **ambiguous branch** (more than one other track touches the
  point — deliberately not guessed at), or reaching a via (vias have no
  `reference`/`number` of their own to report, and aren't walked
  through — a real, documented limitation). `visited` (seeded with the
  track being walked *from*) guards against infinite loops on a
  malformed closed ring.
- Pad/track endpoint touch is decided by exact point equality — the
  same centre-only simplification already used elsewhere in this module
  for via/pad rule-area membership, not a new fuzzy-geometry test.

**Verified:** 3 new `tessera-drc` unit tests reproducing both of the
earlier entry's fixtures directly (the 3-pad daisy chain: each segment
matches only its own real `fromTo` pair, neither matches the
non-adjacent one; the bent 2-pin route: both segments match despite
neither directly touching both pads), plus a new third fixture —
extending the bent route with a third track meeting the bend at a bare
point (still no pad) to make it a genuine 3-way branch — confirming the
walk correctly refuses to guess and no segment resolves a `fromTo`
match there at all. All 40 pre-existing `tessera-drc` tests still pass
unchanged, plus the full `tests/drc_parity.rs` (24 real `kicad-cli`
subprocess invocations) and the corpus-board regression test, both of
which exercise this exact code path against real, non-synthetic
geometry. Full workspace `fmt`/`clippy --all-targets -D warnings`/`test
--workspace` clean throughout — 218 tests total (via `cargo test
--workspace`'s own summary lines), up from 215 before this entry's 3
new tests.

**Still not covered:** via `fromTo` resolution (unchanged, still
2-pin-net-wide); walking through a via to continue a connectivity walk
across a layer change (vias act as a hard stop, not a pass-through);
extending `diff_pair_gap`/`diff_pair_uncoupled`'s coupling to
bent/multi-segment diff pairs, which could now reuse this same walk —
noted as a natural next step, not attempted in this entry.

**This closes the M2.5 `.kicad_dru` gap list this session started
from** (task #19 through #25, minus #23's corpus-blocked corridor
tuning) — every constraint kind the plan's custom-rule engine needs is
now either implemented (`track_width`, `disallow`, `length`,
`clearance`, `hole_size`, `via_diameter`, `diff_pair_gap`,
`diff_pair_uncoupled`) or has its remaining scope precisely documented,
not vaguely "still missing."

## Newer `.kicad_pcb` name-only net references — parsed now; the corpus/tuning attempt that found it stayed blocked

**Date:** 2026-08-17. Picks back up on task #23 (corridor/constant
tuning, left `pending` in the previous entry) after the user offered a
real board of their own (`esp32-plc`, a 4-layer, unrouted design) to
test with.

### What actually happened, in order

1. `kicad-cli 10.0.3` (this session's pinned oracle version) refused to
   load the board at all (`Failed to load board`) — its `.kicad_pcb`
   `(version 20260206)` is newer than 10.0.3 supports.
2. With the user's explicit sign-off, upgraded to `kicad-cli 10.0.4`
   (the latest stable Ubuntu-PPA release) via `apt`. Re-verified the
   existing corpus board still reports the same `0` `"clearance"`-type
   violations under 10.0.4 (extra findings are the same
   `copper_edge_clearance`/`lib_footprint_issues`/`track_dangling`
   categories already documented as unrelated) — the upgrade didn't
   silently drift the ground truth this project's whole empirical
   method depends on, at least for the one thing this crate checks.
3. `10.0.4` **still** failed to load `esp32-plc.kicad_pcb`, even after
   the user re-saved it with a confirmed-matching `10.0.4` GUI (`About
   KiCad` screenshot showing the exact same version). Ruled out project-
   file issues (tested with the `.kicad_pro` present, absent, and
   independently JSON-valid) and DRC-specific issues (`pcb export svg`
   fails identically). This looks like a genuine `kicad-cli` bug against
   this specific file's content, not a version mismatch — not something
   fixable from this project's side. Not pursued further; the user was
   told directly in case they want to report it upstream.
4. Per the user's explicit choice ("skip kicad-cli cross-check, use the
   board anyway"), parsed the file with `tessera-io-kicad` directly. First
   finding: **every net reference in the file — every pad, and (had any
   existed) every track/via — uses a newer, ID-less shape,
   `(net "<name>")`, with no numeric ID and no top-level `(net <id>
   "<name>")` table at all.** `tessera_io_kicad::parser` had only ever
   seen the classic numeric-ID shape; every pad in the file failed to
   parse (`net_id: u32 = ...atom(1)?.parse().ok()?` on a quoted name
   string). Fixed — see "What changed" below.
5. With net parsing fixed, a **second, much bigger blocker** surfaced:
   of the file's 373 pads, only 61 are circular (this crate's only
   modelled pad shape), and those 61 turn out to all be `np_thru_hole`
   mounting holes with **no net at all** — every real, netted component
   pad in the file is `rect` (221) or `oval` (90). With today's
   circular-pads-only scope, essentially none of this board's real
   connectivity parses. This is a materially bigger gap than a parser
   fix (new pad-shape geometry across `tessera-geom` and `tessera-drc`,
   the same scope boundary `pad.rs`'s own doc comment already flags as
   deliberately deferred) — not attempted here.

**Net result for task #23: still `pending`, still blocked** — this
board can't drive corridor/congestion tuning either, for a different
reason than the existing corpus board (that one has no usable pads at
all; this one has pads, but not in a shape this crate models). The
underlying blocker is the same as before: no board available to this
project yet has both (a) circular, parseable pads and (b) multiple real
nets with genuine routing congestion. Not re-attempted with a
third board this session — see the previous entry's reasoning for why
sourcing one more casually isn't the right call.

### What changed

- `tessera_io_kicad::parser::parse_nets` now handles **both** net-
  reference shapes, transparently: the classic top-level `(net <id>
  "<name>")` table (unchanged — `NetId` is exactly KiCad's own ID), and
  a newer shape with no table at all, every reference by name only
  (`NetId` synthesized here, first-seen document order). Detection is
  per-file (whichever shape the first `(net ...)` found has), not
  per-item — the two shapes are never mixed within one real file.
- New `resolve_net_id(net_expr, name_to_id)` — used by `parse_one_pad`/
  `parse_one_track`/`parse_one_via` instead of parsing `atom(1)` as a
  number directly; transparently resolves either shape via the same
  function, no format-specific branching at each call site.
- New `collect_name_only_net_refs`: a genuine recursive tree walk (not
  `Sexpr::find_all`, which is direct-children-only) needed because
  pads are nested inside footprints, at arbitrary depth from the root —
  the name-only shape has no flat top-level table to read instead.

**Verified:** 4 new `tessera-io-kicad` parser unit tests (two pads on
the same net, name-only shape, resolve to the same synthesized `NetId`
and one `Net` entry; two pads on different names get distinct IDs; a
mounting hole with no `(net ...)` field at all is still correctly
skipped, not crashed on or given an arbitrary net). All 80 pre-existing
`tessera-io-kicad` tests pass unchanged. Full workspace `fmt`/`clippy
--all-targets -D warnings`/`test --workspace` clean throughout — 221
tests total (via `cargo test --workspace`'s own summary lines), up from
218 before this entry's 4 new tests.

**Environment note for future sessions on this machine:** `kicad-cli`
is now `10.0.4` (upgraded from the `10.0.3` prior sessions pinned),
re-verified against the existing corpus board with no behavior drift in
what this crate checks. `docs/SESSION_HANDOFF.md` updated to reflect
this as the new pinned version.

## Pad shapes extended beyond circle-only: axis-aligned `Rect` added

**Date:** 2026-08-17, same session as the previous entry. Two
independent real boards this session (the corpus antenna's 2 FPC
connector pads, and the user's `esp32-plc` board — 311 of 373 real
pads) both hit this crate's "circular pads only" scope limit. Rather
than shrug at a third board hitting the same wall, took it on directly:
`tessera-geom` gains an axis-aligned `Rect` shape with exact clearance
predicates, wired all the way through to the parser. Result on the real
board that prompted it: **0 → 213 pads now parse**, with 22 real 2-pin
connections and 15 multi-pin nets recoverable — genuinely usable for
routing/DRC now, not just a shape-count curiosity.

### The geometry, verified before trusting it

`Rect::distance_sq` is the standard clamped-coordinate distance to an
axis-aligned bounding box (`dx = max(min.x-p.x, 0, p.x-max.x)`, same for
`dy`, `i128` throughout, no float, no division) — a well-established
formula, but checked against real `kicad-cli` anyway before wiring it
into `tessera-drc`, matching this project's standing rule: geometry
correctness isn't exempt from empirical verification just because the
math looks textbook.

- A rect pad (2x1mm) and a track whose closest approach is diagonally
  past the pad's corner: `kicad-cli` reports `actual 0.4099mm` —
  matches `Rect::clears_segment`'s corner-distance computation exactly
  (centreline ≈0.5099mm minus the track's 0.1mm half-width).
- Two rects that overlap on one axis but not the other (a common real
  case, not just the corner case): `kicad-cli` reports `actual
  0.0030mm` — the pure single-axis gap, confirming the "if ranges
  overlap on an axis, that axis contributes zero" branch of the AABB
  formula is exactly what KiCad measures too, not an approximation.

### Pad rotation — checked against evidence, not assumed away

A pad's own local rotation (`(at x y ANGLE)`, relative to its
footprint) was previously irrelevant — a circle is rotation-invariant,
so nothing ever read it. For a rect pad it matters. Checked the real
board rather than guessing how common this is: **60 of 213 real rect
pads (≈28%) carry a non-zero local angle**, overwhelmingly 90/270. Only
axis-preserving multiples of 90° are handled (swapping width/height for
90/270, unchanged for 0/180); any other angle is an honest skip (a new
warning), not a mismeasured shape. The *footprint's own* rotation
remains entirely unhandled, same pre-existing documented gap as always
(position was already computed ignoring it) — this doesn't make that
worse, just doesn't fix it either.

### What changed

- `tessera_geom::Rect { center, half_width_nm, half_height_nm }` (new
  file, `rect.rs`): `contains_point`, `distance_sq`, `clears_point`,
  `clears_circle`, `intersects_rect`/`clears_rect`,
  `intersects_segment`/`clears_segment`. The segment case mirrors
  `Segment::clears_segment`'s own reasoning (once intersection is ruled
  out, the minimum gap between two convex straight-edged shapes is
  always at a vertex-to-opposite-shape pairing) with the rect's four
  corners standing in for a second segment's two endpoints.
- `tessera_model::PadShape` gains a `Rect(Rect)` variant alongside
  `Circle`. New `PadShape::center()` — the shape-agnostic connection
  point every non-clearance caller actually wanted (rule-area
  membership lookups, `Board::find_unrouted_connections`'s endpoint
  extraction), replacing what used to be `pad_circle(pad).center`
  everywhere that never actually needed circle-specific geometry.
- `tessera-drc::clearance` replaces the old always-circular `pad_circle`
  helper with three shape-dispatching functions —
  `pad_clears_segment`/`pad_clears_circle`/`pad_clears_pad` — used by
  both this module's net-class clearance checks and
  `custom_rules.rs`'s pairwise `.kicad_dru` `clearance` checks (which
  mirror this module's structure and needed the identical fix).
- `tessera-detail::obstacle::ObstacleShape` gains a matching `Rect`
  variant (`clears_point`/`bounding_box`) so the fine detail router's
  obstacle grid treats rect pads as real obstacles too, not just
  DRC-checkable after the fact — confirmed with a new routing test: a
  connection forced to detour around a rect-pad obstacle, checked
  clearance-clean against `tessera-drc` afterward, the same shape of
  test the existing circular-obstacle case already had.
- `tessera-io-kicad::parser::parse_one_pad` now reads `rect` pad shapes
  (with the rotation handling above); `oval`/`roundrect`/`custom`
  remain unmodelled, an honest skip (no clearance predicate exists for
  them, matching `PadShape`'s own doc comment's stated policy for
  adding shape variants only in lockstep with the geometry existing).
  `tessera-io-kicad::fixture` (the DRC-parity-harness writer) writes
  real `rect`-shaped `.kicad_pcb` pads for boards that have them,
  instead of forcing everything to `circle` regardless of actual shape.

**Verified:** 10 new `tessera-geom` unit tests for `Rect` (containment,
point/circle/rect/segment clearance, including the diagonal-corner and
axis-overlap cases the two `kicad-cli` fixtures above reproduce
directly). 4 new `tessera-drc` unit tests (the two `kicad-cli` fixtures
reproduced exactly at the `check_clearance` level; a mixed circle/rect
pad-pad pair). 5 new `tessera-io-kicad` parser tests (unrotated rect;
90° swaps width/height; 180° doesn't; a non-axis-aligned angle is
skipped with a warning, not mismeasured; an oval pad is skipped, not
approximated as rect or circle). 1 new `tessera-detail` integration
test (rect-pad obstacle detour, clearance-clean result). Full workspace
`fmt`/`clippy --all-targets -D warnings`/`test --workspace` clean
throughout — 240 tests total (via `cargo test --workspace`'s own
summary lines), up from 221 before this entry's 19 new tests, zero
regressions in any pre-existing test (including the real-`kicad-cli`
DRC parity harness and the corpus board test, both of which exercise
the changed `pad_circle`→shape-dispatch code paths against real
geometry, not just synthetic fixtures).

**Still not covered:** `oval`/`roundrect`/`custom`/`trapezoid` pad
shapes (no geometry predicate exists yet, same "add a `PadShape`
variant only in lockstep with the matching exact predicate" policy this
crate has followed since M1); footprint-level rotation (pre-existing,
unchanged); rect-pad `hole_size`/`diff_pair_*` interactions weren't
specifically re-verified (those checks only ever read `Pad::drill_nm`/
`Pad::shape.center()`, both shape-agnostic already, so no change was
needed, but it wasn't re-tested against a dedicated rect-pad fixture
either).

## Footprint rotation applied to pad position and shape (90° multiples)

> **Corrected 2026-10-21:** the *shape* half of this entry is wrong — a
> pad's stored angle in `.kicad_pcb` is already absolute, so composing it
> with the footprint's angle double-rotated every pad on a rotated
> footprint. The *position* transform below remains correct. See "Real-
> board pass, 2026-10-21" below.

**Date:** 2026-08-17, same session as the previous entry. Direct
follow-up: routed the `esp32-plc` board end to end for the first time
(`tessera-cli route`, then rendered via `kicad-cli pcb export svg` —
the first real image this project has produced from a genuine external
board) and re-ran `kicad-cli pcb drc` on the result out of curiosity.
Found 58 `shorting_items` findings. Before assuming they were routing
bugs, checked whether they were actually a **parsing** problem: this
crate has never applied a footprint's own `(at x y ANGLE)` rotation to
its pads at all — not just to shape (task #26's scope), but to
*position* — a pre-existing, always-documented gap that had no visible
consequence until enough real pads existed to notice (0 before this
session).

### Verified before implementing, in three separate steps — each one genuinely needed checking, not just "obviously true"

1. **Which direction/sign does a footprint's rotation move a pad's
   position?** Built a fixture (footprint at `(5, 5, 90)`, pad at local
   `(2, 1)`) and used the same "does a marker pad on a different net
   short against it" trick as the diff-pair/clearance work earlier this
   session: only the candidate at absolute `(6, 3)` — not `(5, 7)` or
   other candidates — actually shorts, pinning down the exact transform
   (`x' = dx·cos θ + dy·sin θ`, `y' = -dx·sin θ + dy·cos θ`, in the
   file's own Y-down coordinates) empirically rather than by guessing a
   rotation-matrix sign convention.
2. **Does a rotated rect pad's `(size W H)` swap axes the way "obviously,
   a rotated rectangle swaps width and height" suggests?** This
   *looked* definitional but wasn't actually checked — verified
   directly: a `(size 2 1)` pad at local angle 90° has its real
   `kicad-cli`-measured X-extent at exactly half of `1` (the *height*
   value), not half of `2` — confirming the swap-for-90/270 approach
   already used for pad-local rotation (task #26) is correct, and
   applies the same way when it's the *footprint's* rotation doing the
   swapping.
3. **Does `kicad-cli`'s DRC even check clearance between two pads of the
   *same* footprint?** A real, live hypothesis raised while
   investigating why the routed board's shorts persisted unchanged after
   this fix — genuinely uncertain (some EDA tools exempt intra-footprint
   geometry from board-level DRC, treating it as the footprint library's
   responsibility). Checked directly with a two-pad, deliberately-
   overlapping single footprint: **`kicad-cli` reports the short anyway**
   — no such exemption exists. This ruled out a specific misdiagnosis
   before it turned into a wrong "explanation" in this entry.

### What changed

- `tessera_io_kicad::parser::rotate_90_multiple(dx, dy, angle_deg)` —
  exact integer rotation for the four axis-preserving multiples of 90°
  only (`(dy, -dx)` for 90°, etc. — no float, no trig at runtime; the
  earlier `angle_deg` helper still does the one-time float parse of the
  file's own angle *string*, unrelated to this). Any other angle: the
  pad is skipped (a warning), same "honest gap, not a mismeasured shape"
  policy as every other rotation case this session.
- `parse_one_pad` now takes `footprint_angle_deg` (read once per
  footprint in `parse_pads`, alongside its existing position read) and
  applies `rotate_90_multiple` to the pad's local offset *before* adding
  the footprint's own absolute position. The **combined** angle
  (footprint's own + the pad's own local angle, already handled in
  task #26) decides `Rect` shape orientation — a pad with no local
  rotation of its own can still end up width/height-swapped purely from
  its footprint being rotated.

**Verified:** 3 new `tessera-io-kicad` parser unit tests (the exact
`(5,5,90)`→`(6,3)` position fixture reproduced directly; a rect pad with
zero local rotation inside a 90°-rotated footprint gets its width/height
swapped from the combined angle alone; a footprint at a non-90-multiple
angle skips *every* one of its pads, including plain circular ones,
since even a rotation-invariant shape can't be *positioned* correctly
without the exact transform). All 88 pre-existing `tessera-io-kicad`
tests pass unchanged. Full workspace `fmt`/`clippy --all-targets -D
warnings`/`test --workspace` clean throughout — 243 tests total (via
`cargo test --workspace`'s own summary lines), up from 240 before this
entry's 3 new tests.

**On the 58 `shorting_items`, honestly: this fix did not eliminate
them** (58 before, 58 after — one`clearance` finding shifted, 7→8,
noise-level). Traced one concrete example: a real ESP32-module
footprint's edge/castellation pads, each `1.5×0.9mm`, local angle 270°,
on a 1.27mm pitch — after the (now twice-verified-correct) swap, each
pad is genuinely `1.5mm` tall against a `1.27mm` pitch, a real 0.23mm
geometric overlap regardless of any convention question. Given the
position transform, the shape-swap transform, and the same-footprint-
still-checked assumption are now all individually confirmed correct
against real `kicad-cli`, this reads as a genuine pre-existing property
of this specific (work-in-progress, explicitly unfinished) board's
footprint — not a parsing bug this session left unfixed. Not chased
further without evidence pointing at tessera's own code; flagged to the
user directly rather than either quietly dropped or wrongly blamed on
the parser.

## Task #23, finally attempted — found and fixed a real determinism bug on the way, corridor half-width left unchanged

**Date:** 2026-08-17, same session as the three entries above. With the
`esp32-plc` board now genuinely usable (rect pads + footprint rotation
both landed), attempted the actual corridor-half-width tuning
experiment task #23 has been blocked on all session. **This board is
the user's own private, unfinished design — used locally for this
experiment only, never committed to the corpus or anywhere in this
repo**, unlike the licence-verified antenna board; the finding below is
reproducible in *method*, not in *exact numbers*, without that board.

### A confound found before it could produce a wrong conclusion

Ran `tessera-cli route` on the same board, same code, five times in a
row to get a baseline before touching the corridor constant — and got
**five different results** (22, 24, 23, 25, 25 of 37 nets routed).
Nothing about the corridor width was varied yet; this was purely
run-to-run noise. Traced it to `Board::find_unrouted_connections`
iterating a `HashMap<NetId, Vec<Endpoint>>` — Rust's default hasher is
randomly seeded per process, so the `Vec<Connection>` it returns (and
`tessera_engine::route_board` routes connections in *exactly* that
order, committing each one's copper as a real obstacle before the next
is attempted) was silently different every run. This is exactly the
non-determinism `docs/SESSION_HANDOFF.md` and `route_board`'s own docs
already named as "not yet deterministic; plan §5.4 requires determinism
once parallel rip-up scheduling exists (M5)" — flagged as a future
requirement, not as something already causing measurable problems
*today*. It clearly was: a ±1.5-net noise band on a 37-net board would
have swamped any real signal from the corridor constant, and comparing
two different constant values without controlling for it would have
risked a confidently-wrong conclusion (plan §12's empirical rigor is
exactly meant to catch this kind of thing before it ships as a
finding).

### The fix

`find_unrouted_connections` now sorts `report.connections` and
`report.multi_pin_nets` by `NetId` before returning — cheap, and turns
"a random order every run" into "a fixed, arbitrary-but-stable order
every run" (still not the *plan-ideal* order — that's M5's scheduling
question — just no longer *random*). `route_board`'s own `report.failed`
listing is sorted too, for the same reason applied to its own output.
Re-ran the same board five times after the fix: **26 routed, 11 failed,
identical every single run.**

### The actual experiment, now meaningful

Swept `CORRIDOR_HALF_WIDTH_NM` (`tessera-detail::router`) across
0.5mm/0.75mm/1.0mm/1.5mm (the current default)/2.0mm/3.0mm/5.0mm, one
deterministic run each (no longer needing repeated trials per value —
that's the whole point of the fix above):

| half-width | 0.5mm | 0.75mm | 1.0mm | **1.5mm** | 2.0mm | 3.0mm | 5.0mm |
|---|---|---|---|---|---|---|---|
| routed / 37 | 23 | 25 | 25 | **26** | 22 | 23 | 23 |

Not a smooth function of the parameter — expected, given a fixed
processing order through a sequential, obstacle-commit-as-you-go
router means a wider or narrower corridor changes *which* path each
net takes, which changes what the *next* net runs into; this isn't a
continuous optimization landscape. **The already-live default (1.5mm)
came out on top** of every value tested, on this one board.

**Conclusion: `CORRIDOR_HALF_WIDTH_NM` is left unchanged.** This is a
real, evidence-based check — not a guess, and not skipped — but it's
one board, not the "full corpus" the constant's own doc comment (and
plan §9.1) really call for. Recorded as "checked, current value holds
up, not yet corpus-validated" rather than either "tuned" (overclaiming
a single data point) or "still untouched" (understating the real work
done). Re-run this exact sweep once the checked-in corpus has more than
one usable multi-net board.

**Verified:** 1 new `tessera-model` unit test
(`connections_and_multi_pin_nets_are_sorted_by_net_id` — nets inserted
deliberately out of order, both `connections` and `multi_pin_nets`
confirmed sorted on output). Full workspace `fmt`/`clippy --all-targets
-D warnings`/`test --workspace` clean throughout — 244 tests total (via
`cargo test --workspace`'s own summary lines), up from 243. This also
closes out task #23 from the original gap list — not with a constant
change, but with a real answer and a real bug fixed along the way.

## The 58 `shorting_items`, resolved: hand-traced, not a tessera bug

> **Corrected 2026-10-21:** this conclusion is wrong — the shorts *were* a
> tessera bug (pad shapes double-rotated, see the footprint-rotation
> entry's correction). The hand-trace below only checked pad *position*,
> which was always correct, and reused the same wrong shape rule, so it
> could not have caught it. After the fix: 0 pad-pad shorts. See "Real-
> board pass, 2026-10-21" below.

**Date:** 2026-08-17, same session, direct follow-up after being asked
directly whether the M2/M3 gap-list work meant this project was
"done" — a fair challenge, and the honest answer included that the
`esp32-plc` board's leftover `shorting_items` findings had only been
*probably* ruled out (three refuted hypotheses, not a proof). Rather
than let that stand as a hedge, traced one concrete case by hand to a
conclusive answer instead of a probabilistic one.

### The trace

Picked one violation — `Net-(J11-DP1)` pad shorting `Net-(J11-DN1)` —
and worked it from the raw `.kicad_pcb` source, not the DRC report.
`J11` is `local_lib:USB-C-SMD_TYPE-C16PIN` (an LCSC-sourced part,
`C393939` — the `local_lib` naming and LCSC reference are the usual
signature of a footprint auto-converted from EasyEDA, a real and known
source of footprint-quality issues, not a hobbyist-drawn footprint).
Footprint at `(103.8624, 102.5, -90°)`; pad `A6` (`DP1`) at local
`(-0.2499, -2.3625, 270°)`, size `0.3×1.15mm`.

Hand-computed through the exact transform from the two entries above
(`rotate_90_multiple` for position, combined-angle swap for shape —
footprint's `-90°` normalizes to `270°`; combined with the pad's own
`270°` gives `540° ≡ 180°`, which does *not* swap `0.3×1.15` since a
half-turn preserves which axis each dimension projects onto):

- Predicted absolute position: `(106.2249, 102.2501)`.
- **Matches the DRC report's reported position exactly, to the last
  digit.**
- With that position and the (correctly unswapped) `0.3×1.15mm` shape,
  `DP1` genuinely overlaps the `DN1`/`DN2` pads by ≈0.65mm given their
  real local layout in this footprint (`DN1`/`DN2` are the connector's
  two flip-orientation-redundant pins, at local `y` positions that
  place them within that overlap distance once rotated).

### Conclusion

This is not a tessera parsing or geometry bug — it's `tessera` correctly
reproducing a real overlap that exists in the source footprint's own
pad geometry. This is a strictly stronger check than the earlier
ESP32-module case (task #26/#27 entries): that one applied the
already-verified position/shape-swap rules by hand-computation to real
pad data and got a plausible, self-consistent overlap, but wasn't
cross-checked against a DRC-reported absolute coordinate the way this
one was — worth being precise about which claim rests on which
evidence, not blurring the two together. Combined with the three
earlier refuted hypotheses (rotation direction, shape-swap direction,
same-footprint DRC exemption) and this exact-coordinate match, the
`shorting_items` findings read as genuine footprint geometry, not a
parser defect. **Upgraded from "probably not a tessera bug" (the
earlier entry's honest hedge) to "traced and confirmed not a tessera
bug" for this specific, representative case** — worth recording as its
own entry precisely because the difference between those two claims
matters, and the earlier entry shouldn't be read as having already
closed this out.

No code change — this entry exists to close a documentation gap, not
implement anything. If this board's `J11` footprint is ever cleaned up
independently (outside this project), re-running the same board would
be the natural way to confirm no *other*, currently-masked issue was
hiding behind it.

## Pad shapes extended further: `Oval` added (task #28)

**Date:** 2026-08-17, same session as the `Rect` pad-shape entry above.
KiCad's `oval` pad shape is common on THT connectors (including real
pads on both the corpus antenna board and the user's own board), and
was still an unsupported/skipped shape after `Rect` landed. Extended
the same way: verify the real geometry against `kicad-cli` first, then
implement in `tessera-geom`, then wire through every downstream layer.

### The geometry question: stadium or ellipse?

KiCad's docs and UI describe an oval pad by a single `(width, height)`
pair, which is ambiguous between two different shapes: a true
stadium/discorectangle (a straight spine swept by a constant-radius
circle — the same "thick segment" model this crate already uses for
tracks), or an ellipse. The two shapes agree exactly on-axis (along the
long axis, both reduce to a simple offset) but disagree off-axis, near
the rounded ends — an on-axis test alone can't distinguish them.

Verified with a diagonal fixture designed specifically to tell the two
apart: a 2×1mm oval pad (spine radius 0.5mm), and a marker point placed
diagonally off-axis from the end cap, at the exact distance a
constant-radius circular cap predicts (not the smaller distance an
ellipse's tighter curvature there would predict). Real `kicad-cli`
reported `actual 0.0750mm` — exactly the circular-cap prediction, not
the ellipse one. This settled it: KiCad's oval is a stadium, not an
ellipse, confirming the shape could be built as a thin wrapper around
`Segment`'s already-exact, already-verified predicates rather than new
geometric math.

### What changed

- `tessera_geom::Oval { spine: Segment, radius_nm, center }` (new file,
  `oval.rs`): `new(center, width_nm, height_nm)` builds the spine along
  the longer axis (ties go horizontal, matching a square oval reducing
  to a circle with an irrelevant zero-length spine direction);
  `clears_point`/`clears_circle`/`clears_segment`/`clears_rect`/
  `clears_oval` are all one-line calls into `Segment`'s or `Rect`'s
  existing predicates with `min_nm` widened by the oval's own
  `radius_nm` (and the other shape's radius, for circle/oval pairs).
  5 unit tests, including a reconstruction of the diagonal-cap fixture
  above with exact integer coordinates (a 3-4-5 triangle scaled by
  120,000, giving an exact 600,000nm marker-to-spine-end distance,
  rather than copying the fixture's rounded-mm coordinates verbatim,
  which would have been off by ~13nm and made the assertion flaky at
  the exact boundary).
- `tessera_model::PadShape` gains an `Oval(Oval)` variant;
  `PadShape::center()` extended.
- `tessera_detail::ObstacleShape` gains a matching `Oval(Oval)` variant
  (`clears_point`, `bounding_box`, and the pad→obstacle conversion
  site all extended) — the fine-grained A* router's obstacle grid now
  respects real oval-pad copper, not a circumscribing circle or rect.
- `tessera_drc::clearance`: `pad_clears_segment`, `pad_clears_circle`
  gain `Oval` arms; `pad_clears_pad` is now a full 9-arm match over
  every `Circle`/`Rect`/`Oval` combination, routing mixed pairs through
  whichever side has the relevant `clears_*` method. New tests:
  oval-vs-track clearance, and mixed oval/rect pad-pad clearance.
- `tessera_io_kicad::parser`: `parse_one_pad` gains an `"oval"` arm,
  mirroring `"rect"`'s exact rotation handling (width/height swap for
  90°/270°, honest skip — not a mismeasured approximation — for any
  other angle). 4 new/rewritten tests, including one confirming
  `roundrect` pads remain genuinely unsupported (not accidentally swept
  up by the oval change).
- `tessera_io_kicad::fixture` (the DRC-parity test-fixture writer)
  gains an `Oval` arm computing KiCad's `(width, height)` pair back out
  of the spine's axis-aligned length plus the sweep diameter — needed
  so future oval-pad DRC-parity fixtures can round-trip through real
  `kicad-cli` the same way `Rect` fixtures already do.

Verified the existing corpus regression (`royalblue54l_nfc_antenna_
is_clearance_clean`) is unaffected: that board's 2 skipped pads are
`roundrect`, not `oval`, so the warning-count assertion doesn't shift.

Same real-board rotation profile as `Rect`: only exact 90°-multiple
angles are handled; anything else is an honest skip with a warning.
Footprint-level rotation (as opposed to the pad's own local angle)
remains the same pre-existing, documented gap it always was.

`cargo fmt --check` clean, `cargo clippy --workspace --all-targets --
-D warnings` clean, full `cargo test --workspace` green (254 passed, 0
failed, up from 244 before this task).

---

## Vias gain real `fromTo` connectivity: `item_facts_for_via`

**Date:** 2026-08-19, a new session (a fresh `discover state` catch-up
after resuming with a clean working tree, `main` already in sync with
`origin/main` — see `docs/SESSION_HANDOFF.md` for that catch-up).
Closes the narrower, correctly-scoped version of a gap the "Multi-pin
`fromTo`/`length` support" entry above had flagged and deliberately not
attempted: `check_via_diameter`/`check_hole_size`/`check_disallow`'s via
handling still built `ItemFacts` via `item_facts_for_net`, which sources
`fromTo`'s endpoints from `connection_endpoints_for` — empty for
anything but a strict 2-pad net. A via on any 3+-pad net therefore never
matched any `fromTo` condition at all, the same limitation
`check_track_width`/`check_disallow`'s *track* handling had before
`item_facts_for_track`/`walk_to_pad_endpoint` fixed it.

**First, a probe to avoid re-verifying an already-settled fact:** before
writing anything, a throwaway unit test checked whether
`walk_to_pad_endpoint` (called per-*track*, as `item_facts_for_track`
already does) already passes through a via junction transparently when
another same-net track happens to touch the same point on a different
layer. It does — the walk's "touching" search has never filtered by
layer, so a track ending exactly at a via's position and another
same-net track starting there on the far layer look identical to a bare
bend vertex to the existing code. This is a pure code-completeness
question, not a new `kicad-cli` semantic question: the underlying rule
("first real pad reached, transparent through non-pad junctions") was
already empirically grounded for tracks in the "`track_width`/
`disallow`'s `fromTo` now walks real per-segment connectivity" entry;
applying it starting from a via's own position instead of a track's end
is reusing that grounding, not guessing at new behaviour.

**What changed:** `item_facts_for_via(board, via, areas)` — unlike a
track, which always has exactly two ends, a via can have any number of
same-net tracks touching its position (a plain 2-layer pass-through has
two; a via at a real branch point can have more). It walks outward from
*every* touching track, one endpoint per branch, via the same
`walk_to_pad_endpoint` used everywhere else, plus checks for a pad
located exactly at the via's own position (a via placed directly under
a through-hole pad). Wired into all three via call sites that previously
used `item_facts_for_net`.

**Verified:** all 48 pre-existing `tessera-drc` tests pass unchanged
*before* any new test was added (confirms this is additive, not a
behaviour change for the 2-pin cases those tests already cover); 2 new
tests — a via on a plain 2-layer pass-through resolves to the same
endpoints the old net-level shortcut already gave for that case, and a
via spliced into one branch of a 3-pad daisy chain resolves only to
that branch's own two endpoints (`fromTo('J1','J2')` matches,
`fromTo('J1','J3')` correctly doesn't, since the walk stops at `J2`'s
real pad). Full workspace `fmt`/`clippy --all-targets -D warnings`/
`test --workspace` clean — 256 tests total, up from 254.

**Still not attempted:** `diff_pair_gap`/`diff_pair_uncoupled`'s
connectivity walk for bent/multi-segment diff pairs (the "Multi-pin
`fromTo`/`length` support" entry's other still-open item) — unrelated to
this via fix, not touched here.

---

## Real-workflow pivot: "route only what's unrouted, keep my manual work" — and multi-layer parsing

**Date:** 2026-08-21, later the same session (`discover state` → this).
The user described their actual intended use directly: they hand-route
some tracks and draw copper pours in KiCad themselves, and want this
tool to autoroute only the rest — without disturbing any of that manual
work. This is `AUTOROUTER_PLAN.md` §7.5.2/§7.5.5's "partial routing"
workflow, not a new idea, but the first time it's been checked against
what actually happens today rather than assumed to work.

**What running the tool on a real board actually showed, before
changing anything:** the user provided a real 4-layer board (kept
local-only under `example/`, added to `.gitignore` — never commit this
without explicit go-ahead; a `git add -A` momentarily staged it by
accident and was caught and reset before committing). Running
`tessera-cli route` against it (output only, input untouched) revealed:
manually-routed tracks were correctly left alone (`find_unrouted_
connections` already only touches nets with no existing geometry), but
inner copper layers (`In1.Cu`/`In2.Cu`) were **entirely invisible** to
the parser — not just unroutable, silently absent from the model
altogether — and the output was `fixture.rs`'s from-scratch minimal
rewrite, which would have dropped the board's 3 real copper zones, its
outline, and everything else not directly modelled.

**Good news found while scoping the fix:** the routing engine itself
(`tessera-detail`/`tessera-global`/`tessera-engine`) has no hardcoded
2-layer assumption anywhere in production code — grepped for it
specifically. It's already parameterized by `board.layers`/
`layer_count()` throughout (the via-move loop in `astar::search`
already iterates `0..map.layer_count()`, not a fixed `0..2`). The
2-layer restriction turned out to be narrowly concentrated in
`tessera-io-kicad`'s parser and writer — an I/O problem, not an engine
redesign.

### Multi-layer parsing

`parse_layers` now reads every `signal`-type layer from `(layers ...)`,
not just `F.Cu`/`B.Cu`, and assigns fresh `LayerId`s in **physical
stackup order** — which is *not* KiCad's own ordinal numbering.
Verified against the real board: `(0 "F.Cu" signal) (4 "In1.Cu" signal)
(6 "In2.Cu" signal) (2 "B.Cu" signal)` — `B.Cu` is always ordinal `2`
regardless of layer count, inner layers are numbered `4, 6, 8, ...` in
between, so a plain numeric sort would put `B.Cu` second (wrong) instead
of last. `layer_id_from_name` (a fixed two-entry match) became
`resolve_layer_id`, threaded through track/via/pad/rule-area parsing via
the same `name -> LayerId` map `parse_layers` now returns — mirroring
the existing `name_to_id` (net) parameter-threading pattern already
established in this file for the earlier name-only-net-reference fix.
Vias' `from_layer`/`to_layer` are no longer hardcoded `LayerId(0)`/
`LayerId(1)` — a through via now spans whatever `F.Cu`/`B.Cu` actually
resolve to for this board's real layer count.

Only `signal`-typed layers are treated as copper — `power`/`mixed`
(KiCad layer types that exist for buried plane layers) haven't been
observed in any board this crate has actually parsed, so they're
deliberately not guessed at.

**Verified:** 3 new parser tests using the real board's exact ordinal
pattern (correct stackup-order sort; a track on an inner layer resolves
to the right `LayerId`; a through via spans the full 4-layer stack, not
a hardcoded 2-layer span). All 94 pre-existing `tessera-io-kicad` tests
pass unchanged. Re-running the CLI against the real board afterward:
routing improved from 25/41 to 26/41 nets (one more net succeeded with
4 layers available instead of 2) — modest, but a genuine, measured
improvement, not just "should help in theory."

**What this doesn't yet fix:** the writer (`fixture.rs`) was still
2-layer-only and now explicitly panics on a 4+-layer board rather than
silently truncating it — see the next entry for why that's actually the
right next problem, not a regression.

---

## The real production commit-back writer: splice, don't rewrite

**Date:** 2026-08-21, immediately following the multi-layer parsing
entry above, same session, step 2 of the plan the user confirmed
("full plan, in order").

`fixture.rs` was never meant to be the answer here — its own doc
comment has said so since M1: "not the production commit-back writer."
Extending it to preserve copper zones, board outline formatting,
stackup, and everything else a real board carries would mean
essentially building a full high-fidelity `.kicad_pcb` writer — a much
bigger, slower-to-trust undertaking than the actual job needs, since
this tool never *modifies* existing content in the first place, only
*adds* new tracks/vias for nets that had none.

**What changed:** `tessera_io_kicad::commit::splice_new_geometry(
original_pcb_text, board, new_tracks, new_vias)` — takes the *original
file's own text*, unparsed and untouched, and inserts only the new
`(segment ...)`/`(via ...)` S-expressions just before the root list's
closing paren. Every other byte survives exactly as KiCad wrote it:
copper pours, outline, silkscreen, UUIDs, stackup, every footprint —
none of it is re-serialized, because none of it needs to change.
`tessera-cli`'s `route_command` now snapshots `board.tracks.len()`/
`board.vias.len()` *before* calling `route_board` (which only ever
`.push()`es new items — confirmed by grepping `route.rs` directly rather
than assuming) and splices the delta in; the companion `.kicad_pro` is
copied byte-for-byte rather than regenerated, since this tool never
touches project settings either.

Net references in spliced items are written name-only (`(net "GND")`),
matching the real board's own convention for *every* existing track/via
(not just pads — checked directly), not a numeric ID. **Not yet
verified against an older file using a numeric `(net N "name")` ID
table** — plausible KiCad accepts name-only references in either file
version, but that's a documented assumption, not a checked fact, until
a board using the older convention is available to test against.

### Verified three separate ways

1. **5 new `commit.rs` unit tests**: a spliced item lands *inside* the
   root list (not appended after EOF), untouched content survives
   exactly, correct layer/net names appear, and a bad `LayerId`
   reference fails closed (`None`) rather than writing something
   malformed.
2. **The existing `tessera-cli` end-to-end test** (drives the real
   compiled binary, checks real `kicad-cli` DRC) still passes clean —
   confirms the splice format is genuinely valid KiCad syntax for a
   real, if synthetic, board.
3. **Against the user's actual 4-layer board directly.** Ran the real
   compiled `tessera-cli route` binary against it: 26/41 nets routed
   (matching the previous entry's improvement from multi-layer
   parsing), output preserved all 3 original copper zones, the board
   outline, and all 81 footprints. Real `kicad-cli` DRC verification on
   this *specific* board is blocked by something unrelated to this
   change: this machine's `kicad-cli` 10.0.3 can't even load the
   *original*, unmodified file — confirmed by running DRC on the
   original directly, which fails identically ("Failed to load board").
   The file declares format version `20260206`, newer than 10.0.3
   understands; `docs/SESSION_HANDOFF.md` already recorded a prior
   session hitting the identical symptom on what looks like the same
   board and needing `kicad-cli` 10.0.4+ to work around it — not a new
   finding, not a tessera bug, just confirmed again independently.
   Instead, verified structural/semantic validity the way that *is*
   available on this machine: `tessera_io_kicad::sexpr::parse` accepts
   the spliced output as well-formed syntax, and re-parsing it as a
   `Board` through this crate's own parser recovers exactly the expected
   shape (1212 tracks, 48 vias, 261 pads with the same skip-warning
   count as the original — meaning nothing new broke), confirming the
   splice didn't corrupt anything even though the real-DRC oracle
   couldn't run.

Full workspace `fmt`/`clippy --all-targets -D warnings`/
`test --workspace` clean — 264 tests total.

**What's still open, in order (per the confirmed plan's step 3):**
investigating the ~90 real-board pads still reported as "unsupported"
that aren't accounted for by the ~10 legitimately-out-of-scope ones
(non-plated mounting holes, one custom-shaped pad) — likely
non-90°-rotation, an existing documented gap, but not confirmed yet.

## Step 3, resolved: the ~90 "unsupported pad" warnings were `"*.Cu"`, not rotation

**Date:** 2026-08-21
**Status:** Decided and shipped

### The hypothesis was wrong — verified before touching code, per this project's own discipline

The standing guess (recorded in the entry above) was non-90°-rotation
pads. Built a standalone diagnostic (`rustc` against the release
`.rlib`s, same technique as the earlier SVG renderer) that replicates
`parse_one_pad`'s rotation/shape checks directly against the real board
and counts *why* each pad would be skipped. Result: only 10 pads hit any
rotation- or shape-based skip — exactly the 9 non-plated holes + 1
custom-shape pad already known and accounted for. Zero rotation-based
skips. The hypothesis was cleanly falsified, not just unconfirmed.

So the real `tessera-cli route` run's warning count (96, not "~10") had
to be coming from something the replica diagnostic didn't check. Ran the
actual compiled binary, diffed its 96 warnings against the 10 the
replica predicted, and went hunting for what else `parse_one_pad` bails
out on. Found it: `pad.find("layers")?.as_list()? ... filter_map(|n|
resolve_layer_id(n, layer_map))` — `resolve_layer_id` is an *exact*
name match against `layer_map`. KiCad itself writes `(layers "*.Cu"
"*.Mask")` on many through-hole pads as shorthand for "spans every
copper layer in the stack", instead of naming each one. `"*.Cu"` isn't a
literal layer name `layer_map` can ever contain, so it silently
resolved to nothing, the pad's whole `layers` list came back empty, and
`parse_one_pad` returned `None` at the `if layers.is_empty()` guard —
correctly-shaped, correctly-rotated, correctly-netted pads, discarded
for a completely unrelated reason.

Counted it directly against the real board's raw text (bracket-balanced
pad-block extraction, not just a `grep`, since the wrong extraction
could double-count text that merely mentions `"*.Cu"` inside a nested
list): **86 pads** use `"*.Cu"`, every one of them `thru_hole` (not
`np_thru_hole`) and every one of them carrying a real, non-zero net —
i.e. actual connector/header pins, exactly the kind of thing a real
board is *made of*, not decorative mounting hardware. 86 + 9 NPTH + 1
custom shape = 96, matching the CLI's actual warning count exactly. This
is a much bigger deal than the rotation hypothesis would have been: it
meant most of this board's through-hole component pins — likely every
header and connector — were invisible to the router entirely, not a
handful of edge-case footprints.

### Fix: `resolve_layer_ids`, expanding `"*.Cu"` before the empty-check

Added `resolve_layer_ids(name, layer_map) -> Vec<LayerId>` alongside the
existing single-layer `resolve_layer_id`: for the literal token `"*.Cu"`
it returns every copper `LayerId` currently in `layer_map` (sorted, so
the result is deterministic — `layer_map.values()` iteration order
isn't — and because `parse_layers` assigns `LayerId`s in physical
stackup order already, sorting by `LayerId` happens to recover that
order too, though no caller actually depends on the order, only
membership). For any other token it just delegates to the existing
single-name lookup wrapped in a 0-or-1-element `Vec`. Wired into both
call sites that had the same shape of bug: `parse_one_pad`'s pad-layers
list (`.flat_map` instead of `.filter_map`, then `sort_unstable` +
`dedup` since a `"*.Cu"` plus an explicit `"F.Cu"` in the same list
would otherwise duplicate an entry) and `parse_one_rule_area`'s
zone/keepout-area layers list, which had the *identical* gap already
called out in its own doc comment ("A wildcard entry like `"*.Cu"` ...
is silently dropped ... a real, if rare, gap") — not rare after all,
just previously unmeasured. `"*.Mask"` (also present in the real board's
`(layers "*.Cu" "*.Mask")` pads) isn't a copper layer this crate models
at all, so it correctly continues to resolve to nothing — the fix only
special-cases the one wildcard token that actually maps onto something
this crate tracks.

### Verified

1. New unit test
   `a_wildcard_star_cu_layer_expands_to_every_copper_layer_on_the_board`:
   a 4-layer synthetic board (same fixture shape as the multi-layer
   parsing tests), one `thru_hole` pad written exactly like the real
   board's convention (`(layers "*.Cu" "*.Mask")`), asserts the parsed
   pad ends up on all four copper `LayerId`s and that `"*.Mask"`
   contributes nothing extra.
2. Full workspace `fmt` clean, `clippy --all-targets -D warnings` clean,
   `cargo test --workspace` clean except one confirmed **pre-existing**
   failure unrelated to this change (`track_track_clearance_matches_kicad`,
   a proptest boundary case at a 1nm gap/clearance rounding edge in the
   `kicad-cli`-parity harness — reproduced identically on `main` *before*
   this change via `git stash`, so not a regression this touched).
3. **Against the real board directly**: warning count dropped from
   96 → 10, exactly the 9 NPTH + 1 custom-shape floor. Re-ran
   `tessera-cli route` end to end — now attempts 38 nets (up sharply from
   before, since dozens of nets that previously had *zero* visible
   endpoints on either side now have real ones), 37 of which currently
   fail to route (0 skipped). That failure count is a new, honest
   finding, not yet diagnosed — most likely real routing congestion on a
   denser, more accurately-represented board now that its actual
   connector pins are visible obstacles/targets for the first time,
   but that's a hypothesis, not yet checked the way this entry's own
   opening paragraph insists on. **Next up, not yet started.**

## Real-board pass, 2026-10-21: pad rotation, router grazing, KiCad's DRC tolerance — and why `kicad-cli` couldn't load the board

**Date:** 2026-10-21. Fresh copy of the user's re-saved `esp32-plc`
board, worked on in `/tmp` only. **Note on history:** this session
started from `c656f1e` on a machine that hadn't pulled the 2026-08-20/21
commits (multi-layer parsing, splice writer, `*.Cu`); it independently
re-found the `*.Cu` bug, then rebased onto them and kept their
`resolve_layer_ids` (more general — every copper layer, not just two).
Numbers below are from the rebased code (4 copper layers, splice writer)
unless marked otherwise.

### 1. Fixture writer: through-hole pads written back single-layer

`fixture.rs` wrote every pad as a single-layer `smd` pad on
`layers.first()`, so a through-hole pad came back out F.Cu-only and
`kicad-cli` DRC of fixture output was blind to B.Cu copper. Fix: a
multi-layer pad with a known circular drill is written as a real
`thru_hole` pad; without one (slotted holes, which `Pad::drill_nm` can't
represent) its copper is written per layer rather than inventing a
drill. New parity test `through_hole_pad_clearance_on_the_back_layer_
matches_kicad`, confirmed to **fail against the old writer**. (`*.Cu`
itself was verified against real `kicad-cli` again here — both F.Cu and
B.Cu copper clearance-checked, `actual 0.1000mm`/`0.0500mm` exactly as
predicted — agreeing with the earlier entry.)

### 2. Pad angles are absolute — shapes were double-rotated

On the real board every footprint at 90° had pads stored at 90°, every
one at 180° pads at 180° — the signature of absolute storage. Verified
with real `kicad-cli`: in a footprint at 90°, a `(size 2 0.5)` pad stored
at angle 0 measures horizontal, at 90 vertical, at 180 horizontal
(marker tracks just outside each candidate edge — probes fully inside
copper report nothing, per the overlap gap in `docs/DRC_PARITY.md`).
Relative storage would flip all three. tessera used `footprint angle +
stored angle` for shape orientation; it now uses the stored angle alone
(the footprint angle still, correctly, rotates the pad's *position*).
Measured on the pre-rebase 2-layer pipeline: this alone took KiCad's DRC
of the routed output from 122 pad-pad shorts + 25 pad-pad clearance
findings to 0, and is what the earlier "58 `shorting_items`" really were
— corrections added to both of those entries. The test that encoded the
wrong composition was replaced by `a_pads_stored_angle_is_absolute_
not_composed_with_the_footprints`.

### 3. KiCad's 500nm clearance tolerance

`tessera-drc` flagged USB-C pad pairs 100nm short of 0.2mm that KiCad
didn't. Measured: KiCad only reports a gap more than 500nm short,
independent of clearance value, `max_error` and rule source — details
and the new edge test in `docs/DRC_PARITY.md`. Applied to `tessera-drc`'s
verdicts only. This is consistent with — though without a saved proptest
seed not proven to be — the "pre-existing" `track_track_clearance_
matches_kicad` failure the "`*.Cu`" entry above recorded at a ~1nm
boundary edge: a gap inside that 500nm window is exactly where old
tessera and KiCad disagreed.

### 4. Router moves could graze obstacles between grid points

With 1–3 fixed, KiCad and `tessera-drc` agreed on exactly 5 remaining
violations (2-layer pipeline), all router output: 45° tracks 1–4µm too
close to a pad. A* only checked that each *cell centre* was clear, never
the straight move between two clear centres. Fix (exact, any shape): the
track obstacle map blocks at `R' = ceil(sqrt(R² + h²))`, `h` = half a
diagonal step; the obstacle point nearest a move is then ≥ `R'` from both
ends, hence ≥ `sqrt(R'² − h²) ≥ R` from every point on it. Via map keeps
`h = 0`. Regression test sweeps a rect and a circle across ~120 sub-cell
offsets asserting every move between unblocked cells clears by exactly
`R`; a companion test shows the old centre-only check fails it.

**Result after rebase (4 layers):** 38 routed / 37 failed (the state the
"`*.Cu`" entry left undiagnosed) → **63 routed / 12 failed**, 10 pads
skipped (9 without a net, 1 `custom`), `tessera-drc` 0 violations on the
full output.

### 5. Why `kicad-cli` couldn't load this board — found

Not the file-format version (10.0.4 fails too, as does the `pcbnew`
Python API). Bisected on a scratch copy: removing every footprint made
it load; 40 of 81 footprints fail even alone, with no structural key
separating them from the passing 41; inside a failing one, removing any
single child didn't help but removing all graphics did; narrowing that
down: **footprint graphics on a layer named `"Rescue"`**, which isn't in
the board's layer table (KiCad's name for items whose layer couldn't be
mapped — these are EasyEDA/LCSC-converted footprints). The GUI tolerates
them; `kicad-cli` silently refuses the whole file. Stripping all 455
`Rescue`-layer footprint graphics from a scratch copy makes the full
4-layer board load. Nothing in the repo does this; it's a manual
pre-step for verification (or a cleanup the user may want in KiCad).

### 6. What the real oracle then showed (4 layers, full board) — open

Routed output vs. input baseline (`kicad-cli` on `Rescue`-stripped
copies): unconnected 223 → 95; silkscreen/library/courtyard findings
unchanged; **new** findings caused by routing, all real gaps:

- **Zones not modelled** — 4 `clearance`: a `+5V` track through the F.Cu
  pour of `Net-(D10-A)` (zone clearance 0.25mm). `tessera-drc` can't see
  this either (0 reported), so it's a parity gap as well as a router one.
- **Pads without a net are dropped** — 13 `hole_clearance`: tracks routed
  straight through mounting hole H8 (NPTH, no net). These must be
  obstacles (copper and/or drill).
- **`hole_to_hole`** — 4: same-net vias from different connections land
  ~0.1mm apart (each connection's grid has its own origin). Not checked
  by `tessera-drc` at all.
- **`annular_width`** — 97: the project's Default net class via
  (0.6/0.3mm, annular 0.15) violates the board's own
  `min_via_annular_width` 0.2; the user's own vias are 0.8/0.4.

`cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D
warnings` clean, `cargo test --workspace` 273 passed / 0 failed.

## Gap B closed: non-plated holes and no-net copper are real obstacles

**Date:** 2026-10-21, right after the real-board pass above. The real
oracle showed 13 `hole_clearance` findings: tracks routed straight
through mounting hole H8, because the parser dropped every pad without a
net — all 9 on that board are `np_thru_hole` (7 mounting holes, 2 USB-C
locating pegs).

**Verified against real `kicad-cli` first:**
- A non-plated hole (pad size = drill) is constrained **only** by the
  board's `min_hole_clearance`, from the drill edge, on every layer:
  with net-class clearance 0.5 and hole clearance 0.1 a 0.15mm gap
  passes; with hole clearance 0.4, 0.15 and 0.3 both fail. Same 500nm
  tolerance as copper clearance (short by 499 passes, 501 fails).
- A non-plated pad *larger* than its drill has real copper (a short with
  a net-A track over the annulus).
- A plated pad without a net is ordinary copper held to the Default
  class's clearance — even against another no-net pad.
- Defaults with no project file: `min_hole_clearance` 0.25mm,
  `min_hole_to_hole` 0.25mm, `min_via_annular_width` 0.1mm. Also
  observed: `hole_to_hole` applies between two vias of the *same* net
  (needed for gap C).

**What changed:**
- `tessera-model`: `NetId::NONE` with `same_net` (no-net is never the
  same net as anything, itself included) and Default-class resolution;
  `Hole`/`Board.npth_holes`; `DesignRules`/`Board.rules` with the
  defaults above. `find_unrouted_connections` ignores no-net pads.
- `tessera-io-kicad`: `np_thru_hole` pads become `Hole`s (plus a no-net
  copper pad when larger than the drill; slotted NPTH drills are skipped
  with a warning); plated pads without a net are kept as no-net copper;
  `board.design_settings.rules` is read from `.kicad_pro`. The fixture
  writer emits holes and these rules.
- `tessera-drc`: `check_hole_clearance` (tracks/vias/pads vs. NPTH
  holes) and `same_net` in both clearance checks.
- `tessera-detail`: holes are `Frozen` obstacles on every layer with a
  `fixed_clearance_nm` of `min_hole_clearance` instead of a net-class
  value.

**Verified:** parity test `npth_hole_clearance_matches_kicad` (non-default
0.3mm rule, gaps straddling the tolerance), router test
`detours_around_a_mounting_hole_by_the_boards_hole_clearance`, parser
and model unit tests. Real board: `hole_clearance` 13 → 0; 67 routed /
8 failed (was 63 / 12). Still out of scope: hole clearance for *plated*
holes (via/PTH drills) — net-class copper clearance around their
annulus dominates on this board; `.kicad_dru` `hole_clearance` rules.

`cargo fmt`/`clippy -D warnings` clean, `cargo test --workspace` 283
passed / 0 failed.

## Gap D closed: routed vias meet the board's own annular-ring rule

**Date:** 2026-10-21. 97 (then 107) `annular_width` findings: the
project's Default net class says via 0.6/0.3mm (ring 0.15) while the
board's own `min_via_annular_width` is 0.2mm; the user's hand-placed vias
are 0.8/0.4. **Decision (user-confirmed):** the router enlarges the
diameter just enough to satisfy the board rule — `Board::via_size_for`
returns `max(class diameter, drill + 2 × min ring)` with the class drill,
here 0.7/0.3. Used by both the detail router's via map and the engine's
commit, so the clearance the search plans with is the via actually
written. Other board via minimums (`min_via_diameter`,
`min_through_hole_diameter`) aren't modelled; the class sizes satisfy
them on this board.

**Verified against real `kicad-cli`:** the ring is compared exactly —
no 500nm tolerance here, a diameter 1nm too small (ring 199,999nm) is
reported. `tessera-drc::check_via_annular_width` matches; parity test
`via_annular_width_matches_kicad` (0.700000 / 0.699999 / 0.690000mm).

**Real board:** `annular_width` 107 → 1 (the user's own pre-existing
0.6/0.3 GND via); 65 routed / 10 failed (67/8 with the smaller via).
286 tests pass.

## Gap C closed: hole-to-hole spacing, any net

**Date:** 2026-10-21. 4 `hole_to_hole` findings: same-net vias from
*different* connections landing ~0.1mm apart — each connection's search
grid has its own origin, and the router skipped its own net's items
entirely.

**Verified against real `kicad-cli`:** `hole_to_hole` applies regardless
of net (two same-net vias; a via and a same-net plated pad), edge to
edge, with the same 500nm tolerance (short by 499 passes, 501 fails);
exactly co-located holes are reported as `holes_co_located` instead.

**What changed:** `tessera-drc::check_hole_to_hole` over every drill
(vias, plated pad drills, non-plated holes), skipping co-located pairs.
In the router, `ObstacleMap::block_drill_conflicts` blocks every cell
where the new via's drill would come within `min_hole_to_hole` of any
existing drill, the routed net's own included — applied to the via map
only (tracks have no drill). **Residual risk, not observed:** two vias
placed within the *same* path are invisible to that static map; it would
take a dive-and-return shorter than ~0.55mm. The engine's integration
tests now also assert `check_hole_to_hole`/`check_via_annular_width` on
their routed boards.

**Verified:** parity test `same_net_hole_to_hole_matches_kicad`, DRC and
grid unit tests. Real board: `hole_to_hole` 1 → 0; 66 routed / 9 failed.
290 tests pass.

## Gap A closed: copper-zone fills are modelled, checked and routed around

**Date:** 2026-10-21. 4 (then 7–8) `clearance` findings: tracks routed
through another net's existing F.Cu pour. Zones weren't modelled at all,
so `tessera-drc` couldn't see it either.

**Verified against real `kicad-cli`:**
- Required distance is **max(zone clearance, net-class clearance)**:
  zone 0.25 vs class 0.2 → 0.25; zone 0.1 vs class 0.3 → 0.3; zone 0.1
  vs class 0.2 with a 0.22 gap → no violation.
- DRC checks the **stored fill**, not the zone outline (an unfilled zone
  crossed by a track reports nothing).
- Same 500nm tolerance.
- **Surprise, found by the new parity test disagreeing:** for files
  older than format `20250210`, KiCad grows each stored fill by half the
  zone's `min_thickness` on load (the legacy stroked-fill
  representation); from `20250210` on, fills are exact. Bisected on the
  version number itself, and checked with a second `min_thickness`
  (0.25 → 0.125, 0.4 → 0.2). Growing a region by `r` lowers every
  distance to it by exactly `r`, so this is modelled exactly as an
  addition to the required clearance (`ZoneFill::inflation_nm`).

**What changed:**
- `tessera-geom::Polygon`: exact `clears_point`/`clears_circle`/
  `clears_segment`/`clears_rect` (filled-region semantics: inside never
  clears; outside measures to the nearest edge — fractured-fill slits lie
  in copper, so they never under-measure) and `bounding_box`.
- `tessera-model`: `ZoneFill` (net, layer, zone clearance, polygon,
  inflation), `Board.zone_fills`, `Board::zone_clearance_nm`. A net with a
  pour now counts as already routed, like one with hand-placed tracks —
  the user's own work is left alone.
- `tessera-io-kicad`: every non-keepout zone's `filled_polygon`s (fills
  with arcs are skipped with a warning, not approximated); the writer
  emits fills with `min_thickness = 2 × inflation` so they round-trip.
- `tessera-drc::check_zone_clearance` (tracks, vias, pads vs. other
  nets' fills on the same layer).
- `tessera-detail`: fills are `Frozen` obstacles,
  `ObstacleShape::Polygon(Arc<Polygon>, inflation)` (so `ObstacleShape`/
  `Obstacle` are `Clone`, no longer `Copy`), with clearance
  `ObstacleClearance::AtLeast(zone clearance)` — the obstacle clearance
  override is now an enum (`NetClass` / `Fixed` for holes / `AtLeast`).

**Verified:** parity test `zone_fill_clearance_matches_kicad` (seven
cases incl. both inflation regimes), router test
`detours_around_another_nets_zone_fill_by_the_zone_clearance`, polygon,
parser and DRC unit tests; on the real board `tessera-drc` matched KiCad's
7 track–zone findings exactly before the router fix.

### Where the real board stands after A–D

Real `kicad-cli` DRC, 4 layers, routed output vs. input: **no category
increases** — every remaining finding (silkscreen, library, courtyard,
co-located holes, the user's own 0.6/0.3 via, GND stitching vias
dangling because zones aren't refilled) was already in the input.
Unconnected items 223 → 94; 65 nets routed, 10 failed; routing takes
2.4s. 300 tests pass.

Still open, noted rather than done: routing completion (10 nets fail —
no rip-up/reroute yet, M5); zones aren't *refilled*, so new tracks never
get thermal connections into a same-net pour; hole clearance for plated
holes and `.kicad_dru` hole rules; two vias within one path vs.
hole-to-hole; `custom` pad shapes.

## Routed tracks straightened into straight and 45° runs

**Date:** 2026-10-21. User feedback on the rendered result, next to their
own hand routing: the autorouted tracks "look like a three-year-old drew
them" — fair. The A* search moves on a 0.1mm grid, and for any direction
that isn't exactly 0/45/90° the cheapest grid path is a staircase of
alternating straight and diagonal steps, which `emit_run_segments` wrote
out as one tiny segment per step.

**Change:** `tessera-detail::smooth::straighten` post-processes each
same-layer run of the A* path: from each vertex it jumps to the farthest
later point reachable by one octilinear segment or two (a 45° leg and a
straight one, either order), tolerating up to 12 unreachable candidates
before settling. Every emitted segment is checked **exactly** against
every obstacle (`ClearanceCheck`: the same `tessera-geom` predicates as
DRC, with clearance resolved per obstacle by the same
`Obstacle::required_clearance_nm` the grid map now also uses), so this
can only shorten and clean up a route, never make it less clear. Vias
stay where A* placed them. Both ends now attach to the real pad centre
instead of the nearest grid point whenever that stub is clear.

**Result on the real board:** new track segments 2,633 → 522; 66 nets
routed / 9 failed; real `kicad-cli` DRC still adds no findings. Tests:
staircase → one bend; a detour stays clear of the obstacle and every
emitted segment is octilinear.

**Next (user's convention, not yet implemented):** on their 4-layer
boards In1.Cu is the VCC plane and In2.Cu the GND plane — signal routing
should stay on the outer layers, and plane nets should drop a via into
their plane instead of being routed as tracks.

## Plane layers: `--plane LAYER=NET`, fanout vias, and plane zones

**Date:** 2026-10-21. The user's 4-layer convention: In1.Cu is the VCC
plane, In2.Cu the GND plane — signals belong on the outer layers only.
The router had been treating the inner layers as ordinary signal layers.
**User decisions:** on `esp32-plc`, VCC = `+3.3V`; tessera should create
the plane zones itself (unfilled, the user fills them with B).

**What changed:**
- `tessera-model`: `Plane { layer, net }`, `Board.planes` (set by the
  caller — a routing choice, not something the file states),
  `Board::signal_layers()`/`is_plane_net()`, and `Board.copper_zones`
  (every copper zone in the file, filled or not, so an existing plane
  zone is never duplicated).
- `tessera-detail`: searches only signal layers (vias stay through);
  `fanout_to_plane(board, pad)` — a Dijkstra search (`astar::nearest`)
  from the pad for the nearest cell where a via fits on every layer,
  keeps hole-to-hole spacing, and stays outside the pad by the net-class
  clearance (no via-in-pad), then the usual straightening.
- `tessera-engine`: plane nets are never routed as tracks; fanouts run
  first (short and local), then signals. A through-hole pad needs no
  fanout (its hole reaches the plane); a surface pad the user already
  wired (a same-net track end or via on it) is left alone.
  `RouteReport` gains `fanouts`/`fanout_failed`.
- `tessera-io-kicad::commit`: `splice_new_geometry` also writes an
  unfilled board-outline zone per new plane, in KiCad 10's own zone shape.
  Thermal gap 0.25mm, not KiCad's 0.5mm default: with 0.5mm, real
  `kicad-cli` reported `starved_thermal` on a 2.54mm-pitch header with
  1.7mm pads — the plane copper left between neighbouring pins was
  thinner than the zone's 0.25mm minimum; 0.25mm matches the user's own
  zones and clears it.
- `tessera-cli route --plane In1.Cu=+3.3V --plane In2.Cu=GND …`.

**Verified:** engine tests (no tracks on plane layers, one fanout for the
surface pad and none for the through-hole one, a user-wired pad left
alone, DRC-clean); an end-to-end test that routes a hand-written 4-layer
board and has real `kicad-cli pcb drc --refill-zones` find **zero**
unconnected items and no copper findings. Real board, refilled zones on
both sides: unconnected 223 → 22, 74 fanouts (0 failed), no new DRC
findings (the user's 6 dangling GND vias now land on the GND plane).
Signal routing on two layers: 54 routed / 20 failed — rip-up/reroute
(M5) is the obvious next lever. 305 tests pass.

## Progressive rip-up and reroute — and pin-escape heuristics tried and dropped

**Date:** 2026-10-22. With signals on two layers the real board routed
54 nets with 20 failing. First, a diagnostic: routing each failed
connection *alone* on the board (no other new tracks) succeeded for 19 of
the 20 — so they were lost to routing order/congestion, not geometry.
Only `Net-(J11-CC2)` fails even alone.

**Rip-up (plan §5.4), `tessera-engine::ripup`:** after the first pass,
for each still-failed connection in order: retry it; failing that, plan
it on a board without this run's own routes, find the own routes in that
plan's way (exact clearance and hole-to-hole checks, deliberately
generous), rip them up, route it, reroute them — and keep the result only
if strictly more connections are routed, otherwise restore the snapshot.
Up to 4 passes while anything improves, at most 6 blockers per attempt
(larger caps/more passes measured: no gain). Plan §7.5.4: the only thing
rip-up can remove is an `OwnRoute` — the tracks/vias this run committed
for one connection — so pre-existing geometry, locked or not, cannot be a
candidate by construction. Fanouts now run *before* global negotiation so
the coarse plan already sees their vias.

**Measured on the real board** (two signal layers, planes on In1/In2):
54 / 20 → **66 routed / 8 failed**; unconnected after zone refill 22 →
10; still no new DRC findings; byte-identical output across runs.
Tests: a one-track channel two nets compete for — passes with rip-up,
**fails with it disabled** (checked); the user's own track blocking the
only way is never removed (§7.5.4).

**Tried and removed — pin escapes for fine-pitch parts.** Looking at
the MCU (LQFP-48, 0.5mm pitch), routes ran through the inside of its pin
ring and around its perimeter, walling off later pins. Three heuristics
were implemented and measured one at a time (temporary switches, same
board): a track keepout over the pin ring's interior → 58 routed;
outward escape stubs reserved for every pin before routing → 59; both →
60; interior keepout + a via-free band around the pins → 61; all three →
58 — every one *worse* than none (66). Removed rather than kept as dead
weight. The underlying problem (greedy shortest paths ignoring the
congestion they cause) is what negotiated congestion at the detail level
would address — the natural next step, not more local heuristics.

307 tests pass.

## Routing style: horizontal/vertical runs with 45° corners, not diagonals

**Date:** 2026-10-22. User feedback: the routes still looked scribbled,
not like routes "should" look. Compared against their own hand routing,
the main difference was long 45° diagonals across the board (the
straightener's two-segment shape had one long diagonal leg), plus
needless bends. Measured on the real board, one change at a time
(temporary env-var switches, removed afterwards):

- **Bend penalty** (A* state now carries the incoming direction; 45° and
  90° turns cost extra, turns sharper than 90° are forbidden — acute
  track corners are never wanted): few fewer segments on its own, ~4×
  slower search.
- **Preferred direction per layer** (F horizontal, B vertical): *more*
  vias (106 → 129–131), no completion gain — dropped.
- **Higher via cost** (up to the equivalent of 10mm of track): signal
  vias barely moved (106 → 96). On two layers nearly every via is forced
  by a crossing, not a cheap choice — dropped.
- **Diagonal penalty** (a diagonal step costs more than the two straight
  steps it replaces): routes become horizontal/vertical with 45° only at
  corners. Same or better completion.

**What changed:** A* — `DIAG_EXTRA` 1000, `BEND_45` 100, `BEND_90` 300,
acute turns forbidden, and a tighter admissible heuristic that counts a
diagonal at the cheaper of itself and two straight steps (17s → 11s on
the real board). Straightener — tries shapes tidiest-first: a straight
run; a 45° run up to 2mm; an L of horizontal and vertical runs with a
0.5mm 45° chamfer (either way round); and only then a long 45° leg.

**Real board:** 67 routed / 7 failed (was 66 / 8), unconnected after
refill 9, no new DRC findings, deterministic; routing 11s. The render is
orthogonal with chamfered corners, the classic routed-board look.
308 tests pass.

## More 45° runs, at the user's request

**Date:** 2026-10-22, right after "Routing style" (tagged `nice001`). The
user asked for more 45° lines. Swept on the real board (temporary
switches, removed): diagonal step extra cost, longest pure-45° run the
straightener takes directly, and chamfer size.

| diag extra | free 45° | chamfer | routed / failed |
|---|---|---|---|
| 1000 (nice001) | 2mm | 0.5mm | 67 / 7 |
| 400 | 2mm | 0.5mm | 66 / 8 |
| 200–500 | 5mm | 1mm | 63 / 11 |
| 600 | 5mm | 1–1.5mm | 67 / 7 |
| **700** | **5mm** | **1mm** | **69 / 5** |
| 700 | 5mm | 1.5–2mm | 65–66 |

Below ~600 the diagonals cut across other nets' paths and completion
drops; the results are sensitive (routing order effects), so the pick is
the measured best, not an interpolation. **Chosen:** `DIAG_EXTRA` 700 (a
diagonal step just dearer than two straight ones), pure 45° runs up to
5mm, 1mm chamfers. Real board: 69 routed / 5 failed, unconnected after
refill 7, no new DRC findings, deterministic, 12s. 308 tests pass.

## Board-edge clearance and keepout areas are now enforced while routing

**Date:** 2026-10-22. Two real gaps from the "what's missing" list: the
detail router ignored the board outline entirely (no
`min_copper_edge_clearance`), and parsed keepout rule areas only fed
`.kicad_dru` condition evaluation, never routing. `tessera-drc` checked
neither. Both verified against real `kicad-cli` before implementing:

- **Edge** (`copper_edge_clearance`): the board's
  `min_copper_edge_clearance` (default 0.5mm when the project doesn't set
  it), measured to the `Edge.Cuts` line's centre line, regardless of net
  (net-class clearance 1.0mm with a 0.7mm gap: no finding), same 500nm
  tolerance; a track crossing the edge measures 0; copper wholly outside
  the board, far from the line, isn't reported.
- **Keepout** (`items_not_allowed`): any overlap of a forbidden item's
  copper (width included) with the area on one of its layers; a 0.05mm
  gap is fine, other layers are free; vias judged separately by the
  area's via flag.

**What changed:** `DesignRules.min_copper_edge_clearance_nm` (read from
`.kicad_pro`); obstacles now say what they block (`Blocks::Both` /
`TracksOnly` / `ViasOnly`) so a keepout can forbid one kind only — the
track map and straightening skip via-only obstacles, the via map skips
track-only ones; every outline edge is a wall obstacle on every layer
(closing the board, so nothing routes off it); every keepout area is an
overlap-only obstacle on its layers. `tessera-drc::check_edge_clearance`
/ `check_keepouts`. The fixture writer emits the outline and keepout
areas, and the project's edge rule.

**Verified:** parity tests `edge_clearance_matches_kicad` (non-default
0.4mm rule, gaps around the tolerance, a crossing track) and
`keepout_overlap_matches_kicad` (crossing, near, overlapping by width,
other layer, via inside); router tests routing around a notch in the
outline and around a keepout. Real board unchanged (69 / 5, no new DRC
findings) — it had no route near the edge and no keepout areas.
314 tests pass.

## A* bookkeeping in flat arrays: 12s → 3.2s, identical result

**Date:** 2026-10-22. Timed with temporary counters (perf is blocked for
unprivileged users on this machine): of 12.0s routing the real board,
11.2s was inside `astar::search` (230 calls), obstacle and map building
under 0.3s. The search kept `g_score`/`came_from` in `HashMap`s keyed by
state — nine times more states since the search tracks the incoming
direction. Now dense arrays over the window (cell × layer × direction,
`StateSpace`), and stale queue entries are skipped instead of
re-expanded. Same costs, same goal test: on the real board the routed
count, signal vias, segment count and total length are all exactly
unchanged; 3.2s. 314 tests pass.

## Grid aligned to a pad; negotiated congestion tried and not kept

**Date:** 2026-10-22. Of the 5 nets still failing on the real board, 4
were MCU pins lost to congestion (each routes alone) and one,
`Net-(J11-CC2)`, failed even alone on an empty board.

**CC2 — a grid-offset bug, fixed.** USB-C pins are 0.3mm wide at 0.5mm
pitch. The search window's grid started at an arbitrary corner, so the
grid point nearest the pin centre could be up to 0.05mm off — enough to
sit inside a neighbouring pin's clearance, leaving the search nowhere to
start. `route_connection` now aligns the grid so the start pad's centre
is exactly a grid point, retrying aligned to the goal pad. Test
`routes_out_of_a_fine_pitch_pin_whose_centre_is_off_grid` — fails on the
old code (checked), passes now.

**Measuring against chaos.** A sequential router on one board is chaotic:
small changes move the routed count by ±3. So variants were compared over
six search-window sizes (3–8mm) instead of a single run. Alignment:
66, 67, 70, 69, 70, 68 (mean 68.3) vs. without: 69, 67, 67, 66, 68, 70
(67.8). The window margin is now 6mm, the middle of the stable 5–7mm
plateau, not a one-off best. Real board: 69 routed / 5 failed (all MCU
congestion now), signal vias 117 → 103, no new DRC findings, unconnected
after refill 7, deterministic, 3.9s. Two tests whose "impassable" walls
assumed the old 3mm window were made taller.

**Negotiated congestion (PathFinder-style history) — tried, removed.**
On top of the first pass and rip-up: per round, plan each failed
connection on a board without this run's own routes, raise a history
cost along that plan, then remove all own routes and route again with
the history in A*'s step costs. Variants measured (rounds up to 5,
history step 1000/3000/5000/20000, failed-first vs. original order,
failed connections exempt from history or not): every round routed 104–
110 connections, never beating the first pass + rip-up's 111. Real
PathFinder routes all nets *at once* allowing overlaps and negotiates the
overlaps away; layering only its history term on a sequential router
with hard obstacles doesn't capture that here. The code was removed. The
genuine version is a router-core rewrite (shared occupancy grid,
per-net footprints), not a tweak.

**Speed note** (separate commit): A* bookkeeping moved from hash maps to
flat arrays — 12s → 3.2s with an identical result. 315 tests pass.

## Negotiated congestion (PathFinder) at the detail level — kept, as one of two strategies

**Date:** 2026-10-22. The proper version of what the previous entry
tried as a tweak. User direction for this step: keep everything generic
— nothing tuned to the one test board.

**How it works.** `tessera-detail::Congestion` is a board-wide grid (0.1mm,
signal layers). Every net's routes mark a *footprint* — the cells where
another net's track centre line would violate clearance (clearance +
track width around tracks; clearance + via radius + half a track width
around vias, on every layer) — counted once per net, so a net's own
edges never collide. `negotiate::plan` re-plans, round by round, every
net that collides, against the board's fixed geometry only, with
`route_connection_with_cost` adding `history + present_factor × covering
nets` to each A* step; a via pays that cost on every layer of its cell
(without this, vias landed in other nets' footprints for free — found by
looking at which nets kept colliding). A collision is a cell where one
net's centre line runs through another's footprint; each round, those
cells gain history and the present factor grows 30%. Searches run without
the global router's corridor: a corridor search succeeds whenever nothing
*hard* blocks it, so a route could never leave a congested corridor.
Rounds don't converge monotonically on a real board, so the plans from
the round with the fewest collisions are used. Committing is exact:
`path_is_legal` checks each plan against everything already on the board
(segments and vias against every obstacle, via hole-to-hole); plans that
fail are routed sequentially instead, then rip-up runs as before.

**Why "one of two strategies".** Neither negotiated nor plain sequential
routing wins everywhere, so `route_board` does both on copies of the board
and keeps whichever routes more connections (sequential on a tie) — never
worse than before, on any board, and still deterministic.

**Measured** (real board, over six search-window sizes so no single lucky
setting decides): sequential 66 / 67 / 70 / 69 / 70 / 68 (mean 68.3);
both-and-keep-better 67 / 70 / 72 / 73 / 71 / 71 (mean 70.7) — better or
equal at every size. At the default: **73 routed / 1 failed** (was 69 /
5), unconnected after zone refill 223 → 3, no new DRC findings,
deterministic; routing takes ~38s (was 4s — the price of the second
strategy). Constants (15 rounds, present 500 → +30%/round, history 1000)
are textbook-shaped, not swept to this board. 317 tests pass.

## Net classes from the project: assignments and patterns

**Date:** 2026-10-22. Every net was parsed as `Default`; KiCad 8+ keeps
net-class membership in the project file (`net_settings`), not the board,
and tessera read only the class definitions. So a power class with wider
tracks or larger clearance was silently ignored — by the router and by
`tessera-drc`.

**Verified against real `kicad-cli`** (two tracks, the DRC message names
the class whose clearance applied):
- a class applies to a net via `netclass_assignments` (a class name, or
  a list) or any matching `netclass_patterns` entry;
- patterns match the **whole** net name, **case-sensitively**, as a
  wildcard (`+*V`, `GN?`, `*5V` match; `5V` doesn't match `+5V`, `GND`
  doesn't match `AGND`, `gnd` doesn't match `GND`) or as a regex
  (`^\+.*V$` matches; `GN.$` doesn't match `AGND`);
- of all classes that apply, the **lowest `priority` number** wins
  (swapping two classes' priorities swaps the outcome), ties going to the
  **alphabetically first** name (independent of list order or which has
  the larger clearance; renaming one class flips it) — an explicit
  assignment has no precedence of its own over a pattern;
- nothing applies → `Default`.

**What changed:** `tessera-io-kicad::netclass` (new; adds the `regex`
crate, MIT/Apache-2.0) reads those rules and the parser sets every net's
class from them. The fixture writer emits each non-Default net's class as
an explicit assignment. Tests: rule unit tests (wildcards, regex,
full-match, case, priority and tie-break, undefined class ignored), a
round trip, a `kicad-cli` parity test for clearance between two classes,
and a routing test (a wider class gets wider tracks, still DRC-clean).
The user's board has only `Default` — its routed output is byte-for-byte
unchanged. 323 tests pass.

## Differential pairs routed side by side; custom polygon pads

**Date:** 2026-10-22. Item 2 of the "what's missing" list, kept generic.

**Which nets pair — verified against real `kicad-cli`** (a `.kicad_dru`
rule on `A.inDiffPair('*')` demanding an impossible width; a finding marks
a pair member). KiCad's rule is broader than the `_P`/`_N`, `+`/`-`
suffixes tessera knew: from the end of the name, skip digits and
underscores; if the next character is `P`, `N`, `+` or `-`, the partner is
the same name with just that character swapped. Pairs: `SIG_P`/`SIG_N`,
`//DP`/`//DN`, `DP1`/`DN1`, `DP12`/`DN12`, `A_P_1`/`A_N_1`, `XP`/`XN`,
`ABP_`/`ABN_`, `D+`/`D-`, `U+_7`/`U-_7`, `P`/`N`. Not pairs: `dp`/`dn`,
`CLKP`/`CLKM`, `XP9`/`XN8`, `QP_`/`QN`. Now `Board::diff_pair_partner`
(model) — also used by `tessera-drc`, whose `inDiffPair` was missing most
of these. `Board::track_width_for` gives pair members the class's
`diff_pair_width`, as KiCad's router does.

**Coupled routing.** After each routing strategy, the second member (higher
net id) of every routed pair of single-connection nets is re-routed with a
`Coupling` cost: free at exactly track width + the class's diff-pair gap
(centre to centre) from the first member's tracks on the same layer,
+200 per 0.1mm off per step, capped at 1000 where the partner isn't near.
It's an ordinary, fully checked route — the cost only steers it — and if
it fails the original is put back. The extra cost is a general
`CostField` trait (congestion implements it too).

**A straightening bug the test exposed:** A* found the coupled path, then
the straightener (which looks only at clearance) replaced it with the
shortest L-shape — the opposite corner, away from the partner (0%
coupled). Now a shortcut is only taken if it costs no more, under the
route's cost field, than the stretch it replaces — which also stops
straightening from undoing negotiated congestion's detours.

**Test:** a pair whose ends are offset diagonally — ≥80% of the second
member runs at 0.45mm ±60µm from the first, at pair width; with coupling
disabled it's 0% (checked). Real board: `//DP`/`//DN` 65% coupled at
0.45mm (the rest is where they fan out to pads at other spacings).

**Custom pads.** The real board's run with this change put a `+5V` track
through U8's thermal pad — a `custom` pad the parser skipped, so invisible
to the router; real `kicad-cli` caught it (`shorting_items`,
`solder_mask_bridge`). A skipped pad is a real hazard, not a cosmetic gap.
`PadShape::Polygon { polygon, center }`, verified against `kicad-cli`
(primitives are pad-local and rotate with the pad's own absolute angle,
like pad positions with a footprint). Accepted only when exactly modelled:
one filled `gr_poly` with no stroke width, `clearance outline`, an anchor
wholly inside the polygon; anything else is still skipped with a warning.
New exact `Polygon::clears_polygon`; every pad-shape dispatch (DRC
clearance, zones, edge, router obstacles, fanout) handles it. Tests:
parser (the real pad verbatim; three unsupported variants skipped), a
`kicad-cli` parity test on an L-shaped (non-convex) custom pad with
violating and clean cases. Real board: parser warnings 1 → 0; routed
72 / 2 (the pad is now a real obstacle), unconnected after refill 4, no
new DRC findings, deterministic. 330 tests pass.
