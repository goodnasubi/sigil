use sigil::parser::parse_std;
use sigil::stdlib::os_sigil::{CONSOLE_SIGIL, FS_PATH_SIGIL};

#[test]
fn test_os_sigil_sources_parse() {
    let c_parsed = parse_std(CONSOLE_SIGIL).expect("parse Console sigil");
    assert_eq!(c_parsed.items.len(), 2);

    let p_parsed = parse_std(FS_PATH_SIGIL).expect("parse Path sigil");
    assert_eq!(p_parsed.items.len(), 2);
}
