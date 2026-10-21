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
capacity model. **Updated a seventh time:** same day, same session —
`diff_pair_gap`/`diff_pair_uncoupled` (deferred earlier this session)
implemented after all, via new `Segment::diff_pair_coupling` geometry.
**Updated an eighth time:** same day, same session —
`track_width`/`disallow`'s `fromTo` now walks real per-segment
connectivity, closing the M2.5 `.kicad_dru` gap list this session
started from. **Updated a ninth time:** 2026-08-17, a new session —
`kicad-cli` upgraded to `10.0.4`; fixed the parser to handle a newer
`.kicad_pcb` name-only net-reference shape, found via a real
user-supplied board. **Updated a tenth time:** same day, same session —
added `PadShape::Rect` (axis-aligned, real clearance predicates
end-to-end) once that same board turned out to be blocked on pad shape,
not net format — unblocks task #23's actual corridor-tuning experiment
(not yet run) as well as growing the corpus generally, since most real
SMD components use rect pads. **Updated an eleventh time:** same day,
same session — actually ran `tessera-cli route` on that board (first
real image this project has rendered from an external board) and, while
investigating leftover DRC findings, found and fixed footprint-level
rotation (position and shape both, 90°-multiples only) — see
`docs/DECISIONS.md`'s "Footprint rotation applied..." entry, including
why the remaining findings on that specific board don't look like a
tessera bug after three separate empirical checks. **Updated a twelfth
time:** same day, same session — task #23 itself, finally: found and
fixed a real routing-order determinism bug along the way (`HashMap`
iteration order was random per process), then swept the corridor
half-width constant now that comparisons were actually meaningful — the
live 1.5mm default won, left unchanged. See `docs/DECISIONS.md`'s "Task
#23, finally attempted..." entry. **Updated a thirteenth and final
time:** same day — asked directly whether this all meant the project
was "done" (it doesn't — see the milestone-by-milestone honest answer
in that conversation, not repeated here), and in the process of
answering honestly, upgraded the `shorting_items` finding from
"probably not a tessera bug" to hand-traced-and-confirmed for one
representative case (exact position match against real footprint data,
by hand) — see `docs/DECISIONS.md`'s "The 58 `shorting_items`,
resolved" entry. **Updated a fourteenth time:** same day, same
session — added `PadShape::Oval` (task #28), the same end-to-end
treatment as `Rect`: verified against real `kicad-cli` first (a
diagonal off-axis fixture confirming KiCad's oval is a true stadium,
not an ellipse), then built as a thin wrapper around `Segment`'s
already-exact predicates and wired through every layer. See
`docs/DECISIONS.md`'s "Pad shapes extended further: `Oval` added"
entry. **Updated a fifteenth time:** 2026-08-19, a new session — asked
to "discover state" after resuming with a clean working tree already in
sync with `origin/main` (see this file's own prior "Environment"/"Where
things stand" sections, which were already current); ran the full
verification suite fresh on this machine (this one's `kicad-cli` is
still `10.0.3`, not the `10.0.4` a prior session upgraded to elsewhere —
harmless, tests pass either way, but the two machines aren't identical)
before doing anything else. Then closed the narrower, real half of the
"walking through a via" gap this file had flagged: `check_via_diameter`/
`check_hole_size`/`check_disallow`'s via handling still used the
2-pin-net-only shortcut, so a via on any 3+-pad net never matched any
`fromTo` condition — fixed via `item_facts_for_via`, after first probing
(a throwaway test, not committed) that the per-track walk already passed
through via junctions correctly on its own, so this needed reusing that
grounding, not re-verifying it against `kicad-cli`. See
`docs/DECISIONS.md`'s "Vias gain real `fromTo` connectivity" entry.
**Updated a sixteenth time:** same session — the user described their
actual real-world use directly (hand-route some tracks, draw copper
pours, autoroute the rest, without touching their manual work) and
provided a real 4-layer board to test against (kept local-only under
`example/`, `.gitignore`d — a `git add -A` briefly staged it by
accident, caught and reset before committing; never commit that
directory without explicit go-ahead). Confirmed and fixed two real gaps
this exposed: the parser silently dropped every layer but `F.Cu`/`B.Cu`
(the routing engine itself needed no changes — already generically
layer-count-parameterized), and the CLI's output was a from-scratch
minimal rewrite that would have dropped the board's copper zones and
everything else not directly modelled. Built the actual production
writer (`tessera_io_kicad::commit::splice_new_geometry`) — inserts only
newly-routed items into the *original* file's own text, everything else
byte-for-byte untouched — verified against the real board directly
(preserved all 3 zones/outline/81 footprints; real `kicad-cli` DRC
verification blocked by an unrelated pre-existing issue, that specific
board needing `kicad-cli` 10.0.4+, already documented above). See
`docs/DECISIONS.md`'s "Real-workflow pivot..." and "The real production
commit-back writer..." entries. **Updated a seventeenth time:** same
session — resolved this file's own standing "~90 unexplained pad
warnings, likely non-90° rotation" open item. The rotation hypothesis
was wrong (a standalone diagnostic replicating `parse_one_pad`'s checks
found zero rotation-based skips against the real board). The real cause:
KiCad writes `(layers "*.Cu" "*.Mask")` on through-hole pads as
shorthand for "every copper layer", and the exact-name-match
`resolve_layer_id` couldn't resolve that literal wildcard string to
anything, so those pads' whole `layers` list came back empty and got
silently discarded — 86 of the real board's pads hit this, every one a
real connector/header pin with a real net, not decorative hardware.
Fixed via a new `resolve_layer_ids` that expands `"*.Cu"` to every
copper `LayerId` on the board, wired into both the pad-layers and
rule-area/zone-layers parsing (the latter had the identical gap already
flagged in its own doc comment as "rare" — it wasn't). Warning count on
the real board dropped from 96 to the expected floor of 10 (9 NPTH + 1
custom shape). See `docs/DECISIONS.md`'s "Step 3, resolved..." entry —
including the new, not-yet-diagnosed finding it surfaced: with those 86
pads now visible, the real board attempts far more nets than before (38,
up from far fewer) and most of them (37) currently fail to route,
plausibly real congestion on a now-more-accurately-modelled dense board,
not yet confirmed. **Updated an eighteenth time:** 2026-10-21, a new
session on a machine that hadn't pulled the 2026-08-20/21 work (it
re-found `*.Cu` independently, then rebased and kept that version).
Found and fixed: pad shapes double-rotated (`.kicad_pcb` pad angles are
already absolute — which **overturns** the earlier "58 `shorting_items`
aren't a tessera bug" conclusion), router moves grazing obstacles between
grid points, the fixture writer dropping through-hole pads' B.Cu copper,
and matched KiCad's 500nm DRC clearance tolerance (very likely the
"pre-existing" proptest failure noted above). Real board: 38/37 → 63/12
routed/failed. Also found why `kicad-cli` can't load that board:
footprint graphics on an undefined `"Rescue"` layer — strip them from a
scratch copy and the full 4-layer board loads, giving a real DRC oracle
for the first time. That oracle then showed four open router gaps
(zones, pads without a net, hole-to-hole, annular ring). See
`docs/DECISIONS.md`'s "Real-board pass, 2026-10-21" entry. Same session,
working through those gaps one commit each: B (non-plated holes and
no-net copper) closed — see "Gap B closed". See "Where things stand" below for the current state,
not the 2026-07-29 snapshot the
rest of this intro paragraph originally described. This file is a
snapshot for picking up work on a different machine, not part of the
permanent design record — `docs/DECISIONS.md` (append-only ADR log with a
milestone-closing entry per milestone) is the authoritative history. Read
that for *why*; this file is only for *what's next* and *what this machine
had that a fresh one might not*.

## Environment this was built against

- **KiCad 10.0.4** (upgraded 2026-08-17 from the `10.0.3` prior sessions
  pinned, with explicit user sign-off — re-verified against the existing
  corpus board with no drift in what this crate checks; see
  `docs/DECISIONS.md`'s "Newer `.kicad_pcb` name-only net references"
  entry), installed locally (`kicad-cli` on `PATH`). Several tests shell
  out to the real `kicad-cli pcb drc` binary as an oracle and will print
  "kicad-cli not found on PATH; skipping..." and pass trivially if it's
  absent — they're real regression tests, not just smoke tests, so
  install KiCad 10.x on any machine continuing this work rather than
  relying on the skip path. Relevant tests: `crates/tessera-io-kicad/tests/drc_parity.rs`,
  `crates/tessera-cli/tests/end_to_end.rs`. **Known `kicad-cli` quirk,
  not a tessera bug:** it refused to load one specific real board
  (a user's own 4-layer design) that its own same-version GUI opened and
  saved fine — ruled out project-file and DRC-specific causes; looks
  like a genuine upstream bug, not pursued further.
- Standard Rust toolchain (edition 2021, clippy pedantic enforced via
  `[workspace.lints]` + `clippy.toml`'s `doc-valid-idents`). On this
  machine, `cargo` itself wasn't on `PATH` in a fresh shell —
  `~/.cargo/bin` existed but needed adding explicitly
  (`export PATH="$HOME/.cargo/bin:$PATH"`); check for the same on any
  future machine before assuming `cargo`/`clippy`/`fmt` are just there.
  `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo test --workspace` all pass clean as of this file's last
  update (283 tests total, counted via the test-runner's own
  summary lines, not estimated) — the full test run takes ~45-55s,
  dominated by the DRC parity harness's real `kicad-cli` subprocess
  invocations. **`kicad-cli` cannot load the user's real 4-layer test
  board as-is** — not a version issue (10.0.4 and the `pcbnew` Python
  API fail too). Cause, found 2026-10-21: footprint graphics on a layer
  named `"Rescue"` that isn't in the board's layer table. Strip every
  `fp_*` item on `(layer "Rescue")` from a scratch copy (never the
  user's file) and `kicad-cli pcb drc` works on the full board — do that
  on both the input and the routed output and compare.
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
  `tessera-model::RuleArea` needs. `Rect` (new, 2026-08-17): axis-aligned,
  exact clearance predicates against point/circle/rect/segment (the
  AABB clamped-distance formula, `i128` throughout) — verified against
  real `kicad-cli` on two fixtures (a diagonal-corner gap, a single-
  axis-overlap gap) before being wired into anything. See
  `docs/DECISIONS.md`'s "Pad shapes extended beyond circle-only" entry.
  `Oval` (new, 2026-08-17, task #28): a stadium/discorectangle (spine
  segment + sweep radius) — verified against real `kicad-cli` with a
  diagonal off-axis fixture that a true ellipse would have measured
  differently, ruling that hypothesis out, before building the whole
  type as thin wrappers around `Segment`'s existing predicates (no new
  clearance math). See `docs/DECISIONS.md`'s "Pad shapes extended
  further: `Oval` added" entry.
- `tessera-model`: board data model (`Layer`, `Net`, `NetClass`, `Track`,
  `Via`, `Pad` — `PadShape::Circle`/`Rect`/`Oval`, all with real
  clearance predicates as of 2026-08-17, see below), `Board` aggregate with
  net-class clearance resolution (`resolved_clearance_nm`) and
  unrouted-connection detection (`find_unrouted_connections`, handles
  2-pin directly and reports 3+-pin nets as raw endpoint groups for the
  caller to decompose — now sorted by `NetId` before returning, 2026-08-17,
  fixing a real run-to-run routing-order determinism bug found while
  finally attempting task #23, see `docs/DECISIONS.md`'s "Task #23,
  finally attempted..." entry). `RuleArea` (new, 2026-07-30): name/polygon-
  outline/layer-set/keepout-flags for a named KiCad rule-area zone — the
  geometry half of what the plan calls a `ProtectedRegion`, deliberately
  not that fuller type yet (see the "Session checkpoint" entry in
  `docs/DECISIONS.md` for exactly why). `Board.rule_areas: Vec<RuleArea>`.
  `Pad` gained `reference`/`number` (2026-07-31, once `fromTo`'s exact
  match format was known) — the owning footprint's designator and the
  pad's own KiCad-native number, both parsed from real `.kicad_pcb`
  syntax. `PadShape::center()` (2026-08-17) is the shape-agnostic
  connection point every non-clearance caller actually needs.
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
  `via_diameter` constraints, verified..." entry. `item_facts_for_via`
  (2026-08-19) gives these two plus `check_disallow`'s via handling real
  `fromTo` connectivity on 3+-pad nets too — walks outward from every
  same-net track touching a via's position (a via can have more than the
  two ends a track always has), reusing `walk_to_pad_endpoint` as-is. See
  `docs/DECISIONS.md`'s "Vias gain real `fromTo` connectivity" entry.
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
  `check_diff_pair_gap`/`check_diff_pair_uncoupled` (2026-08-16, later
  same session) — revisited after being deferred earlier this session,
  now implemented: `tessera_geom::Segment::diff_pair_coupling` (new
  reporting-only geometry, like `length_nm`) computes centreline gap,
  coupled length, and uncoupled length for two parallel segments;
  `coupled_diff_pair_tracks` resolves which track pairs count as coupled
  at all (each net has *exactly one* routed segment, same layer,
  parallel — a bent/multi-segment diff pair has no coupling computed,
  not guessed at). Real surprise found first: unlike `clearance`,
  `diff_pair_gap`/`diff_pair_uncoupled` are **single-subject**
  conditions (`A.inDiffPair(...)` alone, no `B` at all) even though they
  compare two tracks — a pairwise `A.inDiffPair(...) &&
  B.inDiffPair(...)` condition matched nothing at all against real
  `kicad-cli`. See `docs/DECISIONS.md`'s "`diff_pair_gap`/
  `diff_pair_uncoupled` constraints, verified and implemented" entry.
  ~~`check_track_width`/`check_disallow`'s `fromTo` is still 2-pin-only~~
  — resolved 2026-08-16, last item of this session: both now use a new
  `item_facts_for_track`/`walk_to_pad_endpoint` per-segment connectivity
  walk (the mechanism the multi-pin entry above had already found and
  verified but deliberately hadn't wired in yet). Safe to do because the
  walk provably agrees with the old 2-pin lookup on every existing test's
  fixture shape (a straight single segment directly between two pads) —
  confirmed by running the full pre-existing test suite *before* writing
  any new test. Via `fromTo` handling in `check_disallow` is unchanged
  (vias have no single "own segment" to walk from). See
  `docs/DECISIONS.md`'s "`track_width`/`disallow`'s `fromTo` now walks
  real per-segment connectivity" entry — **this closes the M2.5
  `.kicad_dru` gap list this session started from** (every constraint
  kind now either implemented or its remaining scope precisely
  documented, task #23's corpus-blocked corridor tuning aside).
  `clearance.rs` (2026-08-17, a new session) replaced its old always-
  circular `pad_circle` helper with shape-dispatching
  `pad_clears_segment`/`pad_clears_circle`/`pad_clears_pad`, once
  `PadShape::Rect` existed — used by both this module and
  `custom_rules.rs`'s pairwise clearance check, which mirrors this
  module's structure. See `docs/DECISIONS.md`'s "Pad shapes extended
  beyond circle-only" entry. Extended again the same day (task #28) once
  `PadShape::Oval` existed: `pad_clears_pad` is now a full 9-arm match
  over every `Circle`/`Rect`/`Oval` combination. See "Pad shapes
  extended further: `Oval` added" entry. 2026-10-21: every clearance
  verdict (net-class and custom-rule) applies KiCad's measured 500nm
  tolerance (`verdict_threshold_nm`) — see `docs/DRC_PARITY.md`.
- `tessera-io-kicad`: a real S-expression parser (`sexpr.rs`, stress-tested
  against a real 70MB/11-layer board; also now has `parse_all` for a bare
  sequence of top-level forms, not just one root) plus semantic board
  extraction (`parser.rs`, now **any number of copper layers**,
  2026-08-21 — see `docs/DECISIONS.md`'s "Real-workflow pivot..." entry
  for the physical-stackup-vs-KiCad-ordinal subtlety this needed to get
  right) and a fixture writer (`fixture.rs`, scoped for the DRC parity
  harness only — still 2-layer-only, a distinct, narrower gap now than
  before). `commit.rs` (new, same day) is the *actual* production
  commit-back writer: `splice_new_geometry` inserts only newly-routed
  items into the original file's own text, everything else preserved
  byte-for-byte — see `docs/DECISIONS.md`'s "The real production
  commit-back writer..." entry. Scope elsewhere: straight tracks, through
  vias, circular/axis-aligned-rect/oval pads (`rect` added 2026-08-17, `oval`
  added later the same day (task #28) with the same rotation handling —
  see `docs/DECISIONS.md`'s "Pad shapes extended beyond circle-only" and
  "Pad shapes extended further: `Oval` added" entries). Footprint-level
  rotation (also 90°-multiples only) applied too, same day — see
  "Footprint rotation applied to pad position and shape" entry;
  `roundrect`/`custom` pad shapes and non-90°-multiple rotation
  (either level) remain unhandled, an honest skip with a warning.
  2026-10-21: a pad's stored angle is treated as absolute (not composed
  with the footprint's) — verified against real `kicad-cli`; the fixture
  writer writes multi-layer pads back as real `thru_hole` pads. Pads with
  no net are kept as no-net copper (`NetId::NONE`), non-plated pads
  become `Board.npth_holes`, and `.kicad_pro` board rules are read into
  `Board.rules` — see "Gap B closed".
  `docs/DRC_PARITY.md` has one documented
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
  same exact `tessera-geom` predicates `tessera-drc` uses —
  `ObstacleShape` gained `Rect` and `Oval` variants (2026-08-17, `Oval`
  later the same day as task #28) alongside `Circle`/`Segment` so
  rect/oval pads are real routing obstacles, not just DRC-checkable
  after the fact. 2026-10-21: the track obstacle map blocks at a
  move-safe radius (`ceil(sqrt(R² + h²))`, `h` = half a diagonal step)
  so moves *between* clear cell centres can't graze an obstacle either —
  proven by an exhaustive sub-cell sweep test. Takes an
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
  end-to-end against actual KiCad DRC. Output now goes through
  `tessera_io_kicad::commit::splice_new_geometry` (2026-08-21), not
  `fixture.rs` — the CLI's output is your original file with only new
  routing added, not a from-scratch rewrite; the companion `.kicad_pro`
  is copied unchanged rather than regenerated.

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
   ~~`diff_pair_gap`/`diff_pair_uncoupled`~~ — resolved 2026-08-16, later
   the same session: implemented via new `tessera_geom::Segment::
   diff_pair_coupling` geometry, scoped to single-segment-per-net,
   same-layer, parallel diff pairs. Real finding: unlike `clearance`,
   these are single-subject conditions, not pairwise — see
   `docs/DECISIONS.md`'s "`diff_pair_gap`/`diff_pair_uncoupled`
   constraints, verified and implemented" entry.
   ~~`check_track_width`/`check_disallow`'s `fromTo` is 2-pin-only~~ —
   resolved 2026-08-16, the last item of this session: both now walk
   real per-segment connectivity (`item_facts_for_track`/
   `walk_to_pad_endpoint`), verified safe by running the full
   pre-existing test suite before adding anything new. Via `fromTo`
   handling in `check_disallow` is unchanged. See `docs/DECISIONS.md`'s
   "`track_width`/`disallow`'s `fromTo` now walks real per-segment
   connectivity" entry.
   **This closes the M2.5 `.kicad_dru` gap list.** What's left, not
   blocking anything: extending the same connectivity walk to
   `diff_pair_gap`/`diff_pair_uncoupled` for bent/multi-segment diff
   pairs (natural follow-up, not attempted); unverified `A`/`B` pairwise
   binding for constraint types other than `clearance` (unclear whether
   it even applies — `length`/`track_width`/`disallow` are all
   single-subject). ~~Vias were a hard stop for `fromTo` connectivity~~
   — resolved 2026-08-19: `item_facts_for_via` gives
   `check_via_diameter`/`check_hole_size`/`check_disallow`'s via handling
   real per-branch connectivity, the same way tracks already had. See
   `docs/DECISIONS.md`'s "Vias gain real `fromTo` connectivity" entry.
   ~~Task #23 (corridor/constant tuning) blocked on corpus breadth~~ —
   **the actual blocker is resolved now, 2026-08-17, later the same
   session**: tried with a real user-supplied board, found and fixed a
   real parser gap (newer `.kicad_pcb` name-only net references), then
   found the *real* blocker was pad shape (the board's real pads are
   almost entirely non-circular) — see `docs/DECISIONS.md`'s "Newer
   `.kicad_pcb` name-only net references" entry for that investigation.
   Rather than stop there, added `PadShape::Rect` with real clearance
   predicates end-to-end (`tessera-geom`/`tessera-model`/`tessera-drc`/
   `tessera-detail`/`tessera-io-kicad`) — see "Pad shapes extended
   beyond circle-only" entry. Result: **0 → 213 pads now parse** on
   that board (22 real 2-pin connections, 15 multi-pin nets). Then
   footprint-level rotation (position + shape), same day — see
   "Footprint rotation applied..." entry. **Then the actual task #23
   experiment, same day**: swept `CORRIDOR_HALF_WIDTH_NM` against that
   board — but first found and fixed a real determinism bug
   (`find_unrouted_connections`'s `HashMap` iteration order was random
   per process, causing ±4-net run-to-run routing variance that would
   have confounded any comparison). Result: the already-live 1.5mm
   default beat every other value tested (26/37 vs. 22-25 for
   0.5mm–5.0mm) — **left unchanged**, a real evidence-based check, not
   a guess, though still only one board's worth of evidence. See
   `docs/DECISIONS.md`'s "Task #23, finally attempted..." entry. This
   closes the gap list this session started from.
5. **FLUTE port**, if/when it's worth the human-gated procedure (plan
   §11.3: read reference → write a prose explanation → **human reviews
   it** → independent Rust design → implement → differential test). The
   MST heuristic in `tessera-global::steiner` is a deliberate stand-in,
   not a placeholder to feel bad about — upgrading is optional polish, not
   a blocking gap, unless corpus measurement shows Steiner tree quality is
   actually costing completion rate or via count.
6. **Real-workflow gaps, from the user's own real board (2026-08-21) —
   the plan's original 3 steps are now all done.** Multi-layer parsing
   (see `docs/DECISIONS.md`'s "Real-workflow pivot..." entry), the
   production splice-writer ("The real production commit-back
   writer..." entry), and step 3, the ~90 (really 96) "unsupported pad"
   warnings (`"*.Cu"` wildcard layer expansion — "Step 3, resolved..."
   entry) are all committed. **New, not part of the original plan, not
   yet started:** that step-3 fix surfaced a real finding worth
   following up — with the 86 previously-invisible pads now visible,
   the real board attempts 38 nets and 37 of them currently fail to
   route (0 skipped). Not yet diagnosed *why* (`RouteReport::failed`
   only carries `NetId`s, no failure reason — that'd be the first thing
   to add if this gets investigated). Plausible causes, none checked
   yet: genuine routing congestion on a denser, more-accurately-modelled
   board now that real connector/header pins are obstacles and targets
   for the first time; a `PathFinder`/detail-router limitation this
   board is the first real stress test for; or something more mundane
   (net-class lookup, grid resolution) — needs the same
   verify-before-implementing approach as everything else in this file.
   Also still open, lower priority: re-verify the splice-writer against
   real `kicad-cli` DRC once a 10.0.4+ binary is available (this
   session could only verify structurally, via this crate's own parser
   round-tripping the output correctly — see the environment note
   above); plain copper-fill zones still aren't parsed/preserved as
   *data* at all (the splice-writer preserves them as inert original
   text, which is sufficient for the "don't touch my zones" goal, but
   this crate still can't reason about them, e.g. for clearance against
   new routing); `commit::splice_new_geometry`'s name-only net-reference
   assumption is unverified against an older numeric-ID-table file.

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
- **Never `git add`/commit anything under `example/`** without the
  user's explicit go-ahead to publish it — it's their own real,
  unlicensed hardware design, kept local-only and `.gitignore`d
  specifically so a routine `git add -A` can't catch it by accident
  again (see "Real-workflow pivot..." in `docs/DECISIONS.md` for the
  near-miss this session had).
