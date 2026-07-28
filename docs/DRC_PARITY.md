# DRC parity gap list

Required by plan §3.2: every known divergence between `tessera-drc` and
KiCad's real DRC, discovered by the parity harness
(`crates/tessera-io-kicad/tests/drc_parity.rs`) or otherwise, marked
`BLOCKING` / `DEGRADED` / `ACCEPTED`. Append as discovered; this file is not
a design doc, it's a factual log of empirically-verified disagreements.

---

## Overlapping different-net copper produces no `clearance` violation in KiCad

**Status:** `DEGRADED`
**Discovered:** 2026-07-28, while building the M1 parity harness
**KiCad version:** 10.0.3

### What was found

`kicad-cli pcb drc --format json` reports a `"clearance"` violation
correctly when two different-net tracks have a positive gap smaller than
the required clearance (verified: 0.05 mm actual gap vs. 0.1 mm required
→ `"Clearance violation ( clearance 0,1000 mm; actual 0,0500 mm)"`).

Changing only the track offset so the same two tracks' copper **fully
overlaps** (by the same 0.05 mm, i.e. going from gap = +0.05 mm to
gap = −0.05 mm) produces **zero violations of any kind** — not
`"clearance"`, not a distinct "shorting"/"overlap" type, nothing. The full
violation list and `ignored_checks` were inspected directly; no
default-disabled check key corresponds to copper overlap between different
nets either.

### Why this matters, and why it's `DEGRADED` not `BLOCKING`

`tessera-drc::check_clearance` reports a violation for both cases (a
positive-gap-but-insufficient case and a fully-overlapping case), since its
exact geometry predicates correctly treat "distance zero or negative" as
"clearance not satisfied." This means, for this one KiCad quirk,
`tessera-drc` is **more conservative than KiCad**, not less — it flags a
case KiCad's own DRC silently accepts.

For an autorouter this is the safe direction to disagree in: it can only
cause tessera to avoid a geometry KiCad happens to tolerate, never the
reverse (accepting geometry KiCad would reject). It's marked `DEGRADED`
rather than `ACCEPTED` because it's still a real, unexplained gap in
understanding KiCad's DRC engine that should be revisited — full copper
overlap between different nets is a real design error a user would want
flagged, and the fact that KiCad's own DRC misses it is worth understanding
before relying on kicad-cli as an oracle for anything overlap-adjacent
(zone fills, courtyard overlap, etc. may have the same blind spot).

### Current mitigation

The M1 parity harness (`track_track_clearance_matches_kicad`) is
parametrised directly by the true edge-to-edge gap and deliberately
excludes the gap < 0 (overlap) region until this is understood better —
see that test's doc comment. Full-overlap geometry is also not a case a
router that respects clearance during routing should ever produce, so this
exclusion doesn't leave a hole in *routing* correctness, only in the
harness's current test coverage of KiCad's DRC engine itself.

### Follow-up

- Test whether this is specific to tracks, or also true for pad-pad /
  via-via / via-pad overlap (the parity harness currently only covers
  track-track).
- Test whether KiCad's GUI-driven interactive DRC (as opposed to
  `kicad-cli pcb drc`) behaves the same way — if the CLI path has a gap the
  GUI doesn't, that's a different, more specific finding.
- Consider filing a KiCad upstream issue if this reproduces cleanly outside
  this project's fixtures.
