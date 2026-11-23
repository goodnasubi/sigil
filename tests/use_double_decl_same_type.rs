use sigil::effects::Options;
use sigil::project::{check_project, ProjectError};
use sigil::ErrorKind;
use std::path::PathBuf;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("sigil-728-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d.canonicalize().unwrap())
    }
    fn write(&self, rel: &str, src: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, src).unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn use_two_fns_sharing_same_pub_type_with_methods() {
    let t = Tmp::new("two_fns");
    t.write(
        "lib/session.sigil",
        r#"module lib.session
  targets server
  trust none

pub struct SessionRecord {
  id: Int,
}

impl SessionRecord {
  pub fn get_id(self: &SessionRecord) -> Int
    intent "get id"
    requires none
    ensures none
    effects none
  {
    return self.id;
  }
}

pub fn encode_session(s: &SessionRecord) -> Int
  intent "encode"
  requires none
  ensures none
  effects none
{
  return s.get_id();
}

pub fn decode_session(id: Int) -> SessionRecord
  intent "decode"
  requires none
  ensures none
  effects none
{
  return SessionRecord { id: id };
}
"#,
    );

    t.write(
        "app/main.sigil",
        r#"module app.main
  targets server
  trust none

use lib.session.decode_session;
use lib.session.encode_session;

pub fn main() -> Int
  intent "main"
  requires none
  ensures none
  effects none
{
  let s = decode_session(42);
  let id = s.get_id();
  return id + encode_session(&s);
}
"#,
    );

    let res = check_project(&t.0, &t.0.join("app/main.sigil"), &Options::default());
    match res {
        Ok(()) => {}
        Err(ProjectError::Diag { file, error }) => {
            panic!(
                "check failed on {}: {:?} {}",
                file.display(),
                error.kind,
                error.message
            );
        }
        Err(ProjectError::Read { file, msg }) => panic!("read error {}: {msg}", file.display()),
    }
}

#[test]
fn duplicate_methods_in_same_module_still_rejected() {
    let t = Tmp::new("dup_methods");
    t.write(
        "app/main.sigil",
        r#"module app.main
  targets server
  trust none

pub struct Item {
  val: Int,
}

impl Item {
  pub fn get(self: &Item) -> Int
    intent "get 1"
    requires none
    ensures none
    effects none
  {
    return self.val;
  }

  pub fn get(self: &Item) -> Int
    intent "get 2"
    requires none
    ensures none
    effects none
  {
    return self.val;
  }
}

pub fn main() -> Int
  intent "main"
  requires none
  ensures none
  effects none
{
  return 0;
}
"#,
    );

    let res = check_project(&t.0, &t.0.join("app/main.sigil"), &Options::default());
    match res {
        Ok(()) => panic!("duplicate method should be rejected"),
        Err(ProjectError::Diag { error, .. }) => {
            assert_eq!(error.kind, ErrorKind::DuplicateDefinition);
        }
        Err(ProjectError::Read { file, msg }) => panic!("read error {}: {msg}", file.display()),
    }
}
