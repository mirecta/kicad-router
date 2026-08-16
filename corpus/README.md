# Corpus

Real boards for regression testing (plan §9.1), each licence-checked before
being committed here (plan: "Licence-check anything sourced from the
community before committing it"). This is a genuinely partial start, not a
finished corpus — see "Status" below.

## Licensing approach

Every board's licence is recorded in its own directory (a `LICENSE` file
copied from the source project) and summarised in the table below. A board
is only added here once its licence is confirmed compatible with
redistributing it inside this GPL-3.0-or-later repository — permissive
licences (MIT, Apache-2.0, CERN-OHL-P, BSD) are straightforwardly fine;
copyleft-with-share-alike licences are fine as-is (we're not modifying the
board files); **non-commercial clauses (e.g. CC-BY-NC-*) are excluded
outright**, since this repository is meant to be usable by anyone including
commercially (ADR-0001's "fully open tool" rationale), and bundling an
NC-encumbered file inside it would quietly contradict that.

**A real, concrete licensing trap hit while building this tier:** KiCad's
own bundled demo boards (the `kicad-demos` Debian/Ubuntu package —
`vme-wren`, `interf_u`, `custom_pads_test`, and most of the other classic
demos referenced elsewhere in this project's docs for *reading*, e.g. in
`docs/DECISIONS.md`'s ADR-0002) carry a package-level copyright notice
reading plainly **"GPL v2"** with no "or later" clause
(`/usr/share/doc/kicad-demos/copyright` on a Debian/Ubuntu KiCad install).
That reads as GPL-2.0-**only** — the same trap flagged for toporouter/gEDA
in `docs/PROVENANCE.md`, and incompatible with this project's
GPL-3.0-or-later. **None of the classic `kicad-demos` package boards are
in this corpus for that reason**, even though several were read/used
directly (not redistributed) earlier in this project for format
verification — reading a file locally to understand a format is not the
same act as committing a copy of it into this repository.

## Status

Plan §9.1's first tier is "5 trivial 2-layer boards, must be 100%." This
corpus currently has **1**. Filling out the rest (and the plan's larger
tiers — 10 real 4-layer designs, boards with heavy custom DRC rules,
DDR-class length-matched buses, BGA escape, Freerouting-failure cases,
adversarial plane-split/keepout/locked-pre-route boards) is real,
outstanding work, not attempted in one pass — each addition needs the same
licence verification this one got, not a bulk import.

## Boards

| Directory | Source | Licence | Layers | Notes |
|---|---|---|---|---|
| `2layer-trivial/royalblue54l-nfc-antenna` | KiCad's own bundled demos (`kicad-demos` package, `royalblue54L_feather/nfc_antenna/`) — but licensed *separately* from the package's own blanket GPL-v2-only notice via its own `LICENSE` file, see below | **CERN-OHL-P v2** (permissive open hardware licence; verified via the board's own `LICENSE` file, not the package's blanket notice) | 2 (F.Cu/B.Cu) | An NFC antenna coil — genuinely trivial (112 straight track segments, 2 vias, 2 pads). Parses cleanly with `tessera-io-kicad::parser` except the 2 connector pads (non-circular — matches the parser's documented circular-pads-only scope, not a new gap). Verified `0` `"clearance"`-type violations via real `kicad-cli 10.0.3` (it has unrelated `copper_edge_clearance` and `track_dangling` findings — neither is a net-class clearance issue tessera-drc claims to check). Regression test: `crates/tessera-io-kicad/tests/corpus_boards.rs`. |
