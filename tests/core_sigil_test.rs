use sigil::parser::parse_std;
use sigil::stdlib::core_sigil::{LIST_SIGIL, MAP_SIGIL};

#[test]
fn test_core_sigil_sources_parse() {
    let l_parsed = parse_std(LIST_SIGIL).expect("parse List sigil");
    assert_eq!(l_parsed.items.len(), 3);

    let m_parsed = parse_std(MAP_SIGIL).expect("parse Map sigil");
    assert_eq!(m_parsed.items.len(), 3);
}
