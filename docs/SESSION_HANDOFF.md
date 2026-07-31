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
  (`2a3dcf5`) — the full test run takes ~45-55s, dominated by the DRC
  parity harness's 24 real `kicad-cli` subprocess invocations (~45s
  alone).
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
  theoretical coordinate ceiling). `Polygon` (new, 2026-07-30):
  `contains_point` (exact crossing-number test) and `intersects_segment`
  (built on the existing `Segment::intersects`) — the primitive
  `tessera-model::RuleArea` needs.
- `tessera-model`: board data model (`Layer`, `Net`, `NetClass`, `Track`,
  `Via`, `Pad` — circular pads only), `Board` aggregate with net-class
  clearance resolution (`resolved_clearance_nm`) and unrouted-connection
  detection (`find_unrouted_connections`, handles 2-pin directly and
  reports 3+-pin nets as raw endpoint groups for the caller to decompose).
  `RuleArea` (new, 2026-07-30): name/polygon-outline/layer-set/keepout-
  flags for a named KiCad rule-area zone — the geometry half of what the
  plan calls a `ProtectedRegion`, deliberately not that fuller type yet
  (see the "Session checkpoint" entry in `docs/DECISIONS.md` for exactly
  why). `Board.rule_areas: Vec<RuleArea>`. `Pad` gained `reference`/
  `number` (2026-07-31, once `fromTo`'s exact match format was known) —
  the owning footprint's designator and the pad's own KiCad-native
  number, both parsed from real `.kicad_pcb` syntax.
- `tessera-drc`: net-class clearance checking across all six item-pair
  types (track/via/pad × track/via/pad), *plus now* real `.kicad_dru`
  custom-rule checking for one constraint kind:
  `custom_rules::check_track_width` (2026-07-31) builds real `ItemFacts`
  per track from `Board` (net class/name, live diff-pair partner lookup,
  rule-area membership via `RuleArea::outline.intersects_segment`,
  `fromTo`'s terminal pads via the new `Board::two_pin_net_endpoints`,
  scoped to 2-pin nets) and resolves/reports actual `track_width`
  violations. **Deliberately not done yet:** `length` (needs summing a
  whole connection's segments), `clearance` (inherently pairwise, needs
  its own item-*pair* `ItemFacts` shape), `disallow` (no numeric bound to
  compare — a different violation shape entirely), and any other
  bound-shaped kind (`hole_size`, `via_diameter`, `diff_pair_*`) — see
  `docs/DECISIONS.md`'s "First real `.kicad_dru` violation" entry.
- `tessera-io-kicad`: a real S-expression parser (`sexpr.rs`, stress-tested
  against a real 70MB/11-layer board; also now has `parse_all` for a bare
  sequence of top-level forms, not just one root) plus semantic board
  extraction (`parser.rs`) and a fixture writer (`fixture.rs`, scoped for
  the DRC parity harness, not the eventual production commit-back writer)
  — both scoped to 2-layer boards, straight tracks, through vias, circular
  pads, no footprint rotation. `docs/DRC_PARITY.md` has one documented
  KiCad DRC gap found along the way (full copper overlap between
  different nets produces zero KiCad violations — filed as `DEGRADED`,
  not blocking). `dru.rs` (2026-07-30) parses `.kicad_dru` custom
  design-rule files into `DesignRules` — condition expressions are kept
  as raw strings there. `dru_expr.rs` (same session) parses *those*
  strings' own mini-language (`A.NetClass == 'x' && !A.insideArea('y') &&
  A.fromTo('ref-*','ref-*')` — negation and `!=` included, both fixed in
  after re-reading `AUTOROUTER_PLAN.md`'s own examples) into an `Expr`
  AST. `parser.rs` also reads named rule-area zones (`(zone (name ...)
  (keepout ...) (polygon ...))`) into `tessera_model::RuleArea`, and now
  reads each pad's owning footprint reference + own pad number too.
  `dru_eval.rs` (2026-07-31) is the evaluator itself: `eval(Expr,
  ItemFacts) -> bool` plus `resolve_constraint`'s last-wins selection —
  deliberately kept a pure function of `ItemFacts`, independent of
  `Board`; `tessera-drc::custom_rules` (same day) is where it gets wired
  to a real board. See `docs/DECISIONS.md`'s "`Pad.reference`/
  `Pad.number`..." and "First real `.kicad_dru` violation" entries.
- `tessera-detail`: grid octilinear A* router. Obstacle rasterization
  (`obstacle.rs`, `Frozen`/`Movable` distinction per plan §7.5.4) uses the
  same exact `tessera-geom` predicates `tessera-drc` uses. Takes an
  optional `waypoints: &[Point]` hint: when given, `route_connection` now
  tries a search hard-confined to a `CorridorMask` tube around the
  waypoint polyline first, falling back to the old unconstrained
  full-window search if that fails (never a completion-rate regression).
  See "`tessera-detail` now hard-confines waypoint-guided search to a
  corridor" (2026-07-30) in `docs/DECISIONS.md`.
- `tessera-global`: `minimum_spanning_tree` (MST-based Steiner heuristic
  for multi-pin nets — original implementation, explicitly **not** a FLUTE
  port, see below) and `pathfinder::negotiate` (PathFinder negotiated
  congestion, McMurchie & Ebeling, implemented directly from the paper).
  `GlobalGrid` is now obstacle-aware (per-cell `obstruction` reduces flat
  per-layer capacity) — `tessera-engine::route::obstruction_from_board`
  populates it from real board geometry. See the "The global grid is now
  obstacle-aware" ADR entry (2026-07-30) in `docs/DECISIONS.md` for the
  design and what's still not covered (via-edge capacity, board outline —
  no such field exists on `Board` yet).
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
2. ~~**Obstacle-aware global grid**~~ — done 2026-07-30, see
   `docs/DECISIONS.md`'s "The global grid is now obstacle-aware" entry.
   Board outline/edge-cuts still isn't fed in (no such field exists on
   `Board` yet), and via-edge capacity still isn't modelled — both
   explicitly out of scope for that change, not silently dropped.
3. ~~**Hard corridor constraint**~~ — done 2026-07-30, see
   `docs/DECISIONS.md`'s "`tessera-detail` now hard-confines
   waypoint-guided search to a corridor" entry. The corridor half-width
   (1.5mm) is a fixed, untuned constant — a corpus is what's needed to
   check whether it's actually a good value across real boards.
4. **`.kicad_dru` parser + evaluator** — all built and now wired for one
   constraint kind (2026-07-30 through 2026-07-31): the S-expression
   level (`tessera-io-kicad::dru`), the condition mini-language
   (`dru_expr`, including `!`/`!=`), the `insideArea`/`intersectsArea`
   and last-rule-wins findings (verified for both tracks and pads), a
   `Polygon` primitive, a `RuleArea` model with real zone parsing,
   `Pad.reference`/`Pad.number`, the evaluator itself
   (`dru_eval::eval`/`resolve_constraint`), and now
   `tessera-drc::check_track_width` producing real violations against a
   real `Board`. See `docs/DECISIONS.md`'s "Session checkpoint",
   "`Pad.reference`/`Pad.number`...", and "First real `.kicad_dru`
   violation" entries for the full history.
   **Still not done, the actual remaining M2.5 gap:** every constraint
   kind besides `track_width` — `length` (needs summing a whole
   connection's segments, not one item's geometry), `clearance`
   (inherently pairwise, needs its own item-*pair* `ItemFacts`
   construction), `disallow` (no numeric bound — a different violation
   shape), and other bound-shaped kinds (`hole_size`, `via_diameter`,
   `diff_pair_*`) on vias/pads, not just tracks. Also still unverified:
   `A`/`B` pairwise binding for constraint types other than `clearance`,
   and multi-pin (3+-pad) net `fromTo` support (`Board::
   two_pin_net_endpoints` deliberately returns `None` there — see its
   docs for why that's a real, not arbitrary, boundary).
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
- Don't extend `Board::two_pin_net_endpoints` to guess at 3+-pad nets
  (e.g. "just pick the two nearest pads") — the honest answer for a
  multi-pin net's per-segment `fromTo` connectivity needs either
  preserving Steiner-decomposition metadata through routing or computing
  real copper connectivity from geometry; a guess would silently
  misevaluate rather than correctly report "not modelled yet." See that
  method's docs and `docs/DECISIONS.md`'s "First real `.kicad_dru`
  violation" entry.
