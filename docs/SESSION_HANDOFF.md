# Session handoff — resume-from-here notes

**Written:** 2026-07-29, at the end of the session that took the project
from an empty repo through M0–M3. **Updated:** 2026-07-31, at the end of
an extended session that took M3's custom-DRC-rule follow-up work from
zero (only `docs/DECISIONS.md` ADR-0002 findings) through a working,
tested `.kicad_dru` parser + evaluator wired into two real constraint
kinds. **Updated again:** 2026-08-16, resumed on a different machine per
this file's own prior instructions (verified the environment first —
`kicad-cli` present, `cargo` needed `~/.cargo/bin` added to `PATH`
explicitly on this machine, worth checking fresh each time) — added a
third wired constraint kind, `length`, with both of its previously-open
semantic questions verified against real `kicad-cli` first. **Updated a
third time:** same day, same session — added a fourth wired constraint
kind, `clearance`, the first genuinely pairwise one, with all four of its
previously-open semantic questions verified against real `kicad-cli`
first. **Updated a fourth time:** same day, same session — added
`via_diameter` and `hole_size` (the latter needed a real `Pad.drill_nm`
field, a genuine pre-existing model gap, not just an unwired constraint).
**Updated a fifth time:** same day, same session — extended `length` to
3+-pin nets (verified against real `kicad-cli`), and found (but
deliberately didn't yet implement) the real mechanism `track_width`/
`disallow` would need for the same extension. **Updated a sixth time:**
same day, same session — `Board` gained a real `outline: Option<Polygon>`
field, parsed from `Edge.Cuts` graphics and fed into the global grid's
capacity model. See "Where things stand" below for the current state, not the 2026-07-29 snapshot the
rest of this intro paragraph originally described. This file is a
snapshot for picking up work on a different machine, not part of the
permanent design record — `docs/DECISIONS.md` (append-only ADR log with a
milestone-closing entry per milestone) is the authoritative history. Read
that for *why*; this file is only for *what's next* and *what this machine
had that a fresh one might not*.

## Environment this was built against

- **KiCad 10.0.3**, installed locally (`kicad-cli` on `PATH`). Several
  tests shell out to the real `kicad-cli pcb drc` binary as an oracle and
  will print "kicad-cli not found on PATH; skipping..." and pass trivially
  if it's absent — they're real regression tests, not just smoke tests, so
  install KiCad 10.x on any machine continuing this work rather than
  relying on the skip path. Relevant tests: `crates/tessera-io-kicad/tests/drc_parity.rs`,
  `crates/tessera-cli/tests/end_to_end.rs`.
- Standard Rust toolchain (edition 2021, clippy pedantic enforced via
  `[workspace.lints]` + `clippy.toml`'s `doc-valid-idents`). On this
  machine, `cargo` itself wasn't on `PATH` in a fresh shell —
  `~/.cargo/bin` existed but needed adding explicitly
  (`export PATH="$HOME/.cargo/bin:$PATH"`); check for the same on any
  future machine before assuming `cargo`/`clippy`/`fmt` are just there.
  `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo test --workspace` all pass clean as of this file's last
  update (203 tests total, counted via the test-runner's own summary
  lines, not estimated) — the full test run takes ~45-55s, dominated by
  the DRC parity harness's 24 real `kicad-cli` subprocess invocations (~45s
  alone).
- **Visualizing routed output** (came up this session, worth keeping):
  `tessera-cli route` writes via `fixture.rs` — pads/tracks/vias only, no
  board outline or silkscreen (see that crate's scope note below). To
  actually *see* a routed board: `kicad-cli pcb export svg --layers
  F.Cu,B.Cu --mode-single --page-size-mode 2 --fit-page-to-board
  --exclude-drawing-sheet -o out.svg routed.kicad_pcb`, then (SVG isn't
  directly viewable in most agent/chat contexts) convert with
  ImageMagick: `magick -density 300 -background white out.svg out.png`.
  Both `kicad-cli` and `magick`/`inkscape`/`convert` are present on this
  machine; worth checking a fresh machine has them too before assuming
  this works out of the box.
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
  custom-rule checking for **six** constraint kinds. `custom_rules::
  check_track_width` (2026-07-31) builds real `ItemFacts` per track from
  `Board` (net class/name, live diff-pair partner lookup, rule-area
  membership via `RuleArea::outline.intersects_segment`, `fromTo`'s
  terminal pads via the new `Board::two_pin_net_endpoints`, scoped to
  2-pin nets) and resolves/reports actual `track_width` violations.
  `custom_rules::check_disallow` (same day) does the same for tracks and
  vias against `disallow` constraints — using `dru_eval::
  resolve_disallow`, a *distinct* resolution function from
  `resolve_constraint` because `disallow`'s last-wins competition is
  scoped by which item types a rule's `args` actually lists, verified
  against real `kicad-cli` (see `docs/DECISIONS.md`'s "`disallow` wired
  in..." entry — a naive kind-only-scoped last-wins gets this wrong).
  `custom_rules::check_length` (2026-08-16) checks a connection's *total*
  routed length (summed across every track segment on the net, via a new
  `tessera_geom::Segment::length_nm` reporting-only helper) — two
  previously-open semantic questions verified first against real
  `kicad-cli`: `actual` is the sum over the whole connection, reported
  once (not per-segment, unlike `track_width`), and `intersectsArea`/
  `insideArea` for this whole-connection check require *every* segment to
  intersect the area, not just one (the opposite of `track_width`'s
  per-item OR semantics) — see `docs/DECISIONS.md`'s "`length` constraint
  semantics, verified..." entry for the three fixtures that pinned this
  down, including one built specifically to rule out a boundary-touch
  artefact in an earlier ambiguous result. **Not scoped to 2-pin nets**
  (updated same session, see below) — `length` measures a 3+-pin net's
  whole routed length exactly like a 2-pin one, and `fromTo` matches any
  two distinct pads on the net, not just physically-adjacent ones.
  `custom_rules::check_custom_clearance` (2026-08-16) is the first
  genuinely *pairwise* constraint kind: it resolves each of the six
  item-pair types via a new `dru_eval::resolve_constraint_pairwise` (a
  custom rule's bound replaces, not combines with, the net-class default),
  falling back to `Board::resolved_clearance_nm` when no custom rule
  matches. Reuses `clearance.rs`'s geometry/layer-sharing helpers (now
  `pub(crate)`) rather than duplicating them. Four semantics verified
  against real `kicad-cli` first: symmetric `A`/`B` binding regardless of
  which item a rule names first, a single-subject condition matching if
  *either* item qualifies, replace-not-combine, and simple last-wins (like
  `track_width`, not `disallow`'s item-type-scoped variant). See
  `docs/DECISIONS.md`'s "`clearance` constraint semantics, verified..."
  entry.
  `custom_rules::check_via_diameter`/`check_hole_size` (2026-08-16, same
  session) are both single-item bound-shaped kinds like `track_width` — no
  new evaluator machinery needed. `via_diameter` only ever fires for vias;
  `hole_size` fires for **both** vias and through-hole pads, which
  required giving `tessera_model::Pad` a real `drill_nm: Option<i64>`
  field (it had none before — a pad's own drilled hole was a genuine, not
  merely unwired, model gap). See `docs/DECISIONS.md`'s "`hole_size`/
  `via_diameter` constraints, verified..." entry.
  `check_length`'s multi-pin support (2026-08-16, same session) — verified
  against real `kicad-cli` on a 3-pad daisy-chained net that `length`
  measures the whole net's routed length regardless of pin count, and
  `fromTo` matches any two distinct pads on the net rather than requiring
  a direct track between them. `dru_eval::ItemFacts.connection_endpoints`
  is now an owned `Vec<EndpointFacts>` (was a fixed 2-element array) with
  a new `from_to_matches_any_pair` — `check_length` builds it from
  *every* pad on the net (`all_pad_endpoints_for`); `check_track_width`/
  `check_disallow`/`check_via_diameter`/`check_hole_size` are unchanged,
  still 2-pin-only. See `docs/DECISIONS.md`'s "Multi-pin `fromTo`/
  `length` support..." entry — it also documents a *second*, genuinely
  different `fromTo` finding (per-segment/per-via connectivity-graph-walk,
  stopping at the first real pad) needed to extend `track_width`/
  `disallow` the same way, split off as its own task rather than rushed.
  **Deliberately not done: `diff_pair_gap`/`diff_pair_uncoupled`** — both
  need real coupled-segment geometry (matching which portions of two
  diff-paired tracks are routed in parallel, measuring local gap /
  uncoupled run length along that) that `tessera-geom` doesn't have any
  primitives for yet. A materially bigger design problem than every other
  kind in this list, deliberately not rushed into `custom_rules.rs`'s
  existing single-item/pairwise shapes — see that same entry's closing
  section for the full reasoning.
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
  `parse_board_outline` (2026-08-16) reads the board's `Edge.Cuts`
  outline — root-level `gr_line`/`gr_rect` items only (never a
  footprint's own local graphics), chained into a single closed
  `Polygon` regardless of declaration order/direction. Any `gr_arc` on
  `Edge.Cuts` makes the whole outline unbuildable (`None` + a warning,
  not an approximation — this crate models no arc geometry anywhere);
  confirmed a real, common case by re-checking the corpus board, which
  has rounded corners and now produces exactly that warning. See
  `docs/DECISIONS.md`'s "Board outline (`Edge.Cuts`) parsed..." entry.
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
  populates it from real board geometry, and now also from
  `Board.outline` when known (`apply_outline_obstruction`, 2026-08-16:
  every cell outside the outline is fully blocked, on every layer — a
  *soft* global-guidance strengthening only, not hard enforcement at the
  detail-routing level, see `docs/DECISIONS.md`'s "Board outline
  (`Edge.Cuts`) parsed..." entry). See the "The global grid is now
  obstacle-aware" ADR entry (2026-07-30) in `docs/DECISIONS.md` for the
  design and what's still not covered (via-edge capacity).
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
   ~~Board outline/edge-cuts fed in~~ — done 2026-08-16, see that same
   doc's "Board outline (`Edge.Cuts`) parsed..." entry. Via-edge capacity
   still isn't modelled — still out of scope, not silently dropped.
3. ~~**Hard corridor constraint**~~ — done 2026-07-30, see
   `docs/DECISIONS.md`'s "`tessera-detail` now hard-confines
   waypoint-guided search to a corridor" entry. The corridor half-width
   (1.5mm) is a fixed, untuned constant. **Investigated 2026-08-16, found
   genuinely blocked, not just "not done yet":** the one corpus board
   (`royalblue54l-nfc-antenna`) parses to zero pads (both are non-circular
   FPC pads, outside this crate's circular-pads-only scope) and only one
   net even if it did parse — zero usable data points for tuning a
   constant whose whole purpose is balancing against *multi-net*
   congestion. See `docs/DECISIONS.md`'s "Corridor/constant tuning:
   investigated, genuinely blocked..." entry. Real progress here needs
   task #1 (corpus) to actually grow past its current one board, and that
   next board needs circular, parseable pads and more than one net —
   check both before spending time on it, not after.
4. **`.kicad_dru` parser + evaluator** — all built and now wired for two
   constraint kinds (2026-07-30 through 2026-07-31): the S-expression
   level (`tessera-io-kicad::dru`), the condition mini-language
   (`dru_expr`, including `!`/`!=`), the `insideArea`/`intersectsArea`
   and last-rule-wins findings (verified for both tracks and pads), a
   `Polygon` primitive, a `RuleArea` model with real zone parsing,
   `Pad.reference`/`Pad.number`, the evaluator itself
   (`dru_eval::eval`/`resolve_constraint`/`resolve_disallow`), and now
   `tessera-drc::check_track_width` + `check_disallow` producing real
   violations against a real `Board`. See `docs/DECISIONS.md`'s "Session
   checkpoint", "`Pad.reference`/`Pad.number`...", "First real
   `.kicad_dru` violation", and "`disallow` wired in..." entries for the
   full history.
   ~~`length` looked like the natural next one and turned out not to
   be~~ — resolved 2026-08-16: both semantic questions (per-segment vs.
   whole-connection; what area membership means for an aggregate check)
   verified against real `kicad-cli` first, then wired in as
   `check_length`. See `docs/DECISIONS.md`'s "`length` constraint
   semantics, verified..." entry.
   ~~`clearance` (inherently pairwise)~~ — resolved 2026-08-16: the first
   genuinely pairwise kind, needed and got its own `eval_pairwise`/
   `resolve_constraint_pairwise` evaluation path (`eval` is now defined as
   `eval_pairwise` with both sides equal — refactor verified
   behavior-preserving against all pre-existing `dru_eval` tests before
   any new ones were added). See `docs/DECISIONS.md`'s "`clearance`
   constraint semantics, verified..." entry.
   ~~`hole_size`/`via_diameter`~~ — resolved 2026-08-16: both single-item
   bound-shaped kinds like `track_width`. `hole_size` needed a real
   `Pad.drill_nm` field first (a genuine, not merely unwired, model gap —
   see `docs/DECISIONS.md`'s "`hole_size`/`via_diameter` constraints,
   verified..." entry).
   ~~multi-pin (3+-pad) net `fromTo`/`length` support~~ — resolved
   2026-08-16 for `length`: verified against real `kicad-cli` on a 3-pad
   daisy-chained net that `length` measures the whole net regardless of
   pin count, and `fromTo` matches any two distinct pads on the net (not
   just directly-wired ones). `dru_eval::ItemFacts.connection_endpoints`
   is now a `Vec`, not a fixed 2-element array. See `docs/DECISIONS.md`'s
   "Multi-pin `fromTo`/`length` support..." entry.
   **Still not done, the actual remaining M2.5 gap:** `diff_pair_gap`/
   `diff_pair_uncoupled` — both need new `tessera-geom` coupled-segment
   geometry (matching parallel-routed diff-pair track portions, measuring
   local gap / uncoupled length) that doesn't exist yet; a materially
   bigger design problem than every other kind in this list, deliberately
   not rushed — see the `hole_size`/`via_diameter` entry's closing
   section. `check_track_width`/`check_disallow`'s `fromTo` is still
   2-pin-only too — the multi-pin entry above found the real mechanism
   needed (a per-segment/per-via copper-connectivity graph walk, stopping
   at the first real pad) but explicitly split it off as its own task
   rather than rushing a bigger, riskier change into well-tested existing
   code; see that entry's own closing section. Also still unverified:
   `A`/`B` pairwise binding for constraint types other than `clearance`
   (unclear whether it even applies — `length`/`track_width`/`disallow`
   are all single-subject, not pairwise).
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
