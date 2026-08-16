//! Runs `tessera-drc` against real boards in `corpus/` (plan §9.1) rather
//! than only synthetic fixtures — the parity harness
//! (`tests/drc_parity.rs`) proves the clearance predicates agree with
//! `kicad-cli` across a parameter space; this proves the same engine
//! reaches the right answer on an actual, professionally-designed board
//! nobody wrote specifically to exercise this code.
//!
//! Each corpus board directory's licensing is recorded in
//! `corpus/README.md` — verified before the board was committed, per plan
//! §9.1's "licence-check anything sourced from the community."

use std::path::Path;

use tessera_drc::check_clearance;
use tessera_io_kicad::parser::parse_board;

fn corpus_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .canonicalize()
        .expect("corpus/ directory must exist at the workspace root")
}

#[test]
fn royalblue54l_nfc_antenna_is_clearance_clean() {
    let dir = corpus_root().join("2layer-trivial/royalblue54l-nfc-antenna");
    let pcb_text =
        std::fs::read_to_string(dir.join("board.kicad_pcb")).expect("read board.kicad_pcb");
    let pro_text =
        std::fs::read_to_string(dir.join("board.kicad_pro")).expect("read board.kicad_pro");

    let parsed = parse_board(&pcb_text, Some(&pro_text)).expect("parse corpus board");
    // The two non-circular FPC connector pads (see docs/DECISIONS.md's
    // parser scope notes), plus one Edge.Cuts arc segment (this board's
    // outline has rounded corners — arcs aren't supported yet, see
    // parse_board_outline's docs) are the only warnings expected —
    // anything else would mean this board is exercising a gap nobody's
    // looked at yet.
    assert_eq!(
        parsed.warnings.len(),
        3,
        "unexpected warning count, investigate: {:?}",
        parsed.warnings
    );
    assert_eq!(
        parsed.warnings.iter().filter(|w| w.contains("pad")).count(),
        2
    );
    assert_eq!(
        parsed
            .warnings
            .iter()
            .filter(|w| w.contains("Edge.Cuts"))
            .count(),
        1
    );
    assert!(
        parsed.board.outline.is_none(),
        "this board's real outline has arcs, so no outline should have been built"
    );

    assert!(!parsed.board.tracks.is_empty());
    assert!(!parsed.board.vias.is_empty());

    // Ground truth verified directly against real kicad-cli 10.0.3: zero
    // "clearance"-type violations on this board (it does have unrelated
    // findings — board-edge clearance, which tessera-drc doesn't check
    // yet, and dangling-track warnings from the antenna's open ends —
    // neither is a net-class clearance issue).
    let violations = check_clearance(&parsed.board);
    assert!(
        violations.is_empty(),
        "tessera-drc disagrees with kicad-cli on a real board: {violations:?}"
    );
}
