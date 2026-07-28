# Session handoff — resume-from-here notes

**Written:** 2026-07-29, at the end of the session that took the project
from an empty repo through M0–M3. This file is a snapshot for picking up
work on a different machine, not part of the permanent design record —
`docs/DECISIONS.md` (append-only ADR log with a milestone-closing entry
per milestone) is the authoritative history. Read that for *why*; this
file is only for *what's next* and *what this machine had that a fresh one
might not*.

## Environment this was built against

- **KiCad 10.0.3**, installed locally (`kicad-cli` on `PATH`). Several
  tests shell out to the real `kicad-cli pcb drc` binary as an oracle and
  will print "kicad-cli not found on PATH; skipping..." and pass trivially
  if it's absent — they're real regression tests, not just smoke tests, so
  install KiCad 10.x on any machine continuing this work rather than
  relying on the skip path. Relevant tests: `crates/tessera-io-kicad/tests/drc_parity.rs`,
  `crates/tessera-cli/tests/end_to_end.rs`.
- Standard Rust toolchain (edition 2021, clippy pedantic enforced via
  `[workspace.lints]` + `clippy.toml`'s `doc-valid-idents`). `cargo fmt`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` all pass clean as of the last commit
  (`0fefc41`) — the full test run takes ~50s, dominated by the DRC parity
  harness's 24 real `kicad-cli` subprocess invocations (~45s alone).
- No other special local state: no corpus files, no scratch directories
  left behind (temp fixture dirs under `/tmp/tessera-*` are cleaned up by
  the tests themselves, or manually between runs — see below).
- One housekeeping note: test runs create `/tmp/tessera-drc-parity-*` and
  `/tmp/tessera-cli-e2e-*` scratch directories; they clean up on success
  but can accumulate if a run is interrupted. Harmless, just `rm -rf` them
  if `/tmp` gets noisy.

## Where things stand

**M0, M1, M2: closed**, exit criteria verified (see `docs/DECISIONS.md` for
each). **M3: substantial progress, explicitly not closed** — its exit
criterion needs a real 4-layer corpus to measure completion rate against,
and none exists yet. Don't mark M3 done without that measurement actually
happening; see the M3 status entry in `docs/DECISIONS.md` for the full
honest gap list.

### What's implemented, crate by crate

- `tessera-geom`: exact i64-nanometre `Point`/`Vector`, `orient` predicate,
  clearance predicates for point/segment/circle/segment-segment pairs.
  Bounded to `MAX_COORDINATE_NM` (1m/axis) — see ADR-0004 for why (a real
  overflow bug, caught by proptest, that's smaller than KiCad's own
  theoretical coordinate ceiling).
- `tessera-model`: board data model (`Layer`, `Net`, `NetClass`, `Track`,
  `Via`, `Pad` — circular pads only), `Board` aggregate with net-class
  clearance resolution (`resolved_clearance_nm`) and unrouted-connection
  detection (`find_unrouted_connections`, handles 2-pin directly and
  reports 3+-pin nets as raw endpoint groups for the caller to decompose).
- `tessera-drc`: net-class clearance checking across all six item-pair
  types (track/via/pad × track/via/pad). **No custom DRC rule (`.kicad_dru`)
  evaluation yet** — that needs the parser (not built) before any
  expression-evaluator work can start; see ADR-0002.
- `tessera-io-kicad`: a real S-expression parser (`sexpr.rs`, stress-tested
  against a real 70MB/11-layer board) plus semantic board extraction
  (`parser.rs`) and a fixture writer (`fixture.rs`, scoped for the DRC
  parity harness, not the eventual production commit-back writer) — both
  scoped to 2-layer boards, straight tracks, through vias, circular pads,
  no footprint rotation. `docs/DRC_PARITY.md` has one documented KiCad DRC
  gap found along the way (full copper overlap between different nets
  produces zero KiCad violations — filed as `DEGRADED`, not blocking).
- `tessera-detail`: grid octilinear A* router. Obstacle rasterization
  (`obstacle.rs`, `Frozen`/`Movable` distinction per plan §7.5.4) uses the
  same exact `tessera-geom` predicates `tessera-drc` uses. Takes an
  optional `waypoints: &[Point]` hint (added this session) that widens its
  local search window's bounding box to follow a global-router-suggested
  corridor — a **soft** influence, not a hard constraint.
- `tessera-global`: `minimum_spanning_tree` (MST-based Steiner heuristic
  for multi-pin nets — original implementation, explicitly **not** a FLUTE
  port, see below) and `pathfinder::negotiate` (PathFinder negotiated
  congestion, McMurchie & Ebeling, implemented directly from the paper).
  The global grid's capacity model is **flat per-layer, not obstacle-aware**
  — it reflects congestion among the nets being routed, not real board
  geometry. This is the biggest concrete gap toward the ≥90% completion
  target.
- `tessera-engine`: `route_board` orchestrates ingest → gather connections
  (direct + Steiner-decomposed) → global negotiation → per-connection
  detailed routing with waypoint hints → commit. Sequential, single-pass,
  no rip-up/reroute (that's M5).
- `tessera-cli`: `route <in.kicad_pcb> <out.kicad_pcb>` — real, tested
  end-to-end against actual KiCad DRC.

### Concrete next steps (in roughly the order they'd naturally come up)

1. **Corpus.** Blocked three milestones' exit criteria in a row now (M1's
   "full corpus" parity claim, M2's rip-up-trap *corpus board*, M3's
   completion-rate target). Even the plan's modest first tier (§9.1: "5
   trivial 2-layer boards") would unblock real measurement instead of only
   synthetic test boards. Check licences on anything sourced from the
   community before committing it (plan §9.1).
2. **Obstacle-aware global grid** — feed real board geometry (locked
   items, existing tracks, board edge) into `GlobalGrid`'s capacity model
   instead of the current flat per-layer constant, the way
   `tessera-detail::ObstacleMap` already does at the fine grid. This is
   what would make the global router's waypoints actually route *around*
   things, not just negotiate relative position among competing nets.
3. **Hard corridor constraint** (optional follow-up to #2) — confine
   `tessera-detail`'s search to a tube around the global path rather than
   just widening its bounding box, once there's a reason to believe the
   global path is trustworthy enough to constrain against (i.e., after #2).
4. **`.kicad_dru` parser + custom DRC rule evaluator** — needed for M2.5
   (protected regions) per ADR-0002; not started. `docs/DECISIONS.md`
   ADR-0002 has the empirically-verified syntax
   (`(rule "name" (constraint ...) (condition "..."))`) and the
   `insideArea`/`intersectsArea` semantic-inconsistency flag to verify
   per-item-type once this is built.
5. **FLUTE port**, if/when it's worth the human-gated procedure (plan
   §11.3: read reference → write a prose explanation → **human reviews
   it** → independent Rust design → implement → differential test). The
   MST heuristic in `tessera-global::steiner` is a deliberate stand-in,
   not a placeholder to feel bad about — upgrading is optional polish, not
   a blocking gap, unless corpus measurement shows Steiner tree quality is
   actually costing completion rate or via count.

### Things a fresh session should NOT do

- Don't re-run the M0 prior-art survey or KiCad API probe — `docs/PRIOR_ART.md`
  and the ADR-0002/ADR-0003 findings in `docs/DECISIONS.md` are current as
  of KiCad 10.0.3 and were verified empirically, not guessed.
- Don't add per-file license headers or otherwise touch the GPL-3.0-or-later
  setup (`COPYING`, `NOTICE`, ADR-0001) unless something's actually wrong
  with it.
- Don't attempt a FLUTE port without the human-review gate — see #5 above.
- Don't mark M3 "closed" without an actual corpus completion-rate measurement.
