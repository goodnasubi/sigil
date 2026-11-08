use sigil::parser::parse_std;
use sigil::stdlib::os_sigil::FS_PATH_SIGIL;

#[test]
fn test_os_sigil_sources_parse() {
    let p_parsed = parse_std(FS_PATH_SIGIL).expect("parse Path sigil");
    assert_eq!(p_parsed.items.len(), 2);
}
