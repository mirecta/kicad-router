//! Cross-checks the `.kicad_dru` parser against the actual shipped demo
//! file on a machine with KiCad installed, not just the verbatim excerpt
//! embedded in `dru.rs`'s unit tests — belt-and-suspenders for the same
//! file ADR-0002 (`docs/DECISIONS.md`) ground-truthed the grammar
//! against. Skips gracefully (like `drc_parity.rs`'s `kicad-cli`
//! availability check) rather than failing when the demo file isn't
//! present, since it's a real system, not the repository, so it isn't
//! guaranteed to exist on every machine running this test suite.

use std::path::Path;

use tessera_io_kicad::dru::parse_design_rules;

const DEMO_DRU_PATH: &str = "/usr/share/kicad/demos/vme-wren/vme-wren.kicad_dru";

#[test]
fn parses_the_real_shipped_demo_file_with_no_warnings() {
    if !Path::new(DEMO_DRU_PATH).exists() {
        eprintln!("{DEMO_DRU_PATH} not found; skipping real-demo-file check");
        return;
    }

    let text = std::fs::read_to_string(DEMO_DRU_PATH).expect("demo file should be readable");
    let parsed = parse_design_rules(&text).expect("demo file should be valid .kicad_dru syntax");

    assert!(
        parsed.warnings.is_empty(),
        "unexpected warnings parsing the real demo file: {:?}",
        parsed.warnings
    );
    assert_eq!(parsed.design_rules.version, Some(1));
    assert_eq!(parsed.design_rules.rules.len(), 11);
}
