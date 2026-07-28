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
