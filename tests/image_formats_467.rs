//! ADR 0035 / 6-2c。通常モジュールを変更せず評価器とネイティブで照合する。
mod common;
const MODULES: &[&str] = &[
    include_str!("../tools/image/binary.sigil"),
    include_str!("../tools/image/elf.sigil"),
    include_str!("../tools/image/ustar.sigil"),
    include_str!("../tools/image/oci.sigil"),
];
const USES: &str = "use tools.image.binary.le;\nuse tools.image.binary.span;\nuse tools.image.binary.zeros;\nuse tools.image.elf.validate_elf;\nuse tools.image.oci.Descriptor;\nuse tools.image.oci.config_json;\nuse tools.image.oci.descriptor;\nuse tools.image.oci.descriptor_digest;\nuse tools.image.oci.descriptor_json;\nuse tools.image.oci.descriptor_media_type;\nuse tools.image.oci.descriptor_size;\nuse tools.image.oci.digest;\nuse tools.image.oci.index_json;\nuse tools.image.oci.layout_json;\nuse tools.image.oci.manifest_json;\nuse tools.image.oci.read_descriptor;\nuse tools.image.oci.read_json;\nuse tools.image.oci.verify_descriptor;\nuse tools.image.ustar.TarFile;\nuse tools.image.ustar.append_file;\nuse tools.image.ustar.archive_size;\nuse tools.image.ustar.finish_tar;\nuse tools.image.ustar.read_tar;\nuse tools.image.ustar.tar_data;\nuse tools.image.ustar.tar_mode;\nuse tools.image.ustar.tar_name;\n";
fn source(signature: &str, body: &str) -> String {
    let uses = USES
        .lines()
        .filter(|line| {
            let name = line.trim_end_matches(';').rsplit('.').next().unwrap();
            body.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|word| word == name)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("module image_test.main\n  targets server\n  trust none\n{uses}\npub fn {signature}\n  intent \"形式試験\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}")
}
fn sources(s: &str) -> Vec<&str> {
    let mut sources = vec![s];
    loop {
        let mut added = false;
        for module in MODULES {
            let name = module.lines().next().unwrap().trim_start_matches("module ");
            if !sources.contains(module)
                && sources
                    .iter()
                    .any(|src| src.contains(&format!("use {name}.")))
            {
                sources.push(module);
                added = true;
            }
        }
        if !added {
            return sources;
        }
    }
}
fn run(body: &str) {
    let s = source("main() -> Result<Int, Int>", body);
    let sources = sources(&s);
    assert_eq!(
        common::run_sources(&sources, "main", &[]).unwrap().value,
        "Ok(0)"
    );
}
#[test]
fn pure_modules_and_known_digests() {
    for module in MODULES {
        assert!(module.contains("trust none"));
        assert!(!module.contains("unchecked"));
        assert!(!module.contains("raw."));
    }
    run(r#"
  if digest(&Bytes.new()) != "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" { return Ok(1); }
  if digest(&"abc".as_bytes()) != "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" { return Ok(2); }
  let d = descriptor(&"application/vnd.oci.image.layer.v1.tar", &"abc".as_bytes())?;
  verify_descriptor(&read_descriptor(&descriptor_json(&d)?)?, &"abc".as_bytes())?;
  if verify_descriptor(&d, &"abd".as_bytes()).is_ok() || verify_descriptor(&d, &"ab".as_bytes()).is_ok() { return Ok(3); }
  return Ok(0);"#);
}
fn bytes(name: &str, data: &[u8]) -> String {
    let mut s = format!("let mut {name} = Bytes.new();\n");
    for b in data {
        s.push_str(&format!("{name}.push({b});\n"));
    }
    s
}
fn elf_fixture() -> Vec<u8> {
    let mut b = vec![0; 120];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    b[16..18].copy_from_slice(&2u16.to_le_bytes());
    b[18..20].copy_from_slice(&62u16.to_le_bytes());
    b[20..24].copy_from_slice(&1u32.to_le_bytes());
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[52..54].copy_from_slice(&64u16.to_le_bytes());
    b[54..56].copy_from_slice(&56u16.to_le_bytes());
    b[56..58].copy_from_slice(&1u16.to_le_bytes());
    b[64..68].copy_from_slice(&1u32.to_le_bytes());
    b[96..104].copy_from_slice(&120u64.to_le_bytes());
    b[104..112].copy_from_slice(&120u64.to_le_bytes());
    b
}
#[test]
fn elf_known_headers_lengths_and_overflow() {
    let good = elf_fixture();
    let mut cases = vec![
        (good.clone(), true),
        (good[..63].to_vec(), false),
        (good[..119].to_vec(), false),
    ];
    for (offset, value) in [
        (0, 0),
        (4, 1),
        (5, 2),
        (6, 0),
        (16, 3),
        (18, 3),
        (20, 0),
        (52, 63),
        (54, 55),
        (56, 0),
        (64, 2),
        (64, 3),
        (32, 63),
        (32, 65),
        (56, 255),
        (57, 255),
        (96, 121),
        (104, 119),
    ] {
        let mut b = good.clone();
        b[offset] = value;
        cases.push((b, false));
    }
    for offset in [32, 72, 80, 96, 104, 112] {
        for value in [i64::MAX as u64, 1u64 << 63, u64::MAX] {
            let mut b = good.clone();
            b[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            cases.push((b, offset == 104 && value == i64::MAX as u64));
        }
    }
    // 二つ目のprogram headerも検査する。
    let mut second = good.clone();
    second.resize(176, 0);
    second[56] = 2;
    second[120] = 3;
    cases.push((second, false));
    let mut body = String::from("if !span(9223372036854775807, 9223372036854775807, 0) || span(9223372036854775807, 9223372036854775807, 1) || span(10, 0 - 1, 1) { return Ok(99); }\n");
    for (i, (b, valid)) in cases.iter().enumerate() {
        body.push_str(&bytes(&format!("b{i}"), b));
        body.push_str(&format!(
            "if validate_elf(&b{i}).is_ok() != {valid} {{ return Ok({}); }}\n",
            i + 1
        ));
    }
    body.push_str("return Ok(0);");
    run(&body);
}
#[test]
fn ustar_boundaries_canonical_bytes_and_corruption() {
    run(r#"
  if archive_size(0, 0)? != 512 || archive_size(0, 512)? != 1024 || archive_size(0, 513)? != 1536 { return Ok(1); }
  if archive_size(0, 9223372036854775807).is_ok() || archive_size(9223372036854775807, 1).is_ok() || archive_size(0, 8589934592).is_ok() || archive_size(0, 0 - 1).is_ok() { return Ok(2); }
  if archive_size(134216192, 0)? != 134216704 || archive_size(134216704, 0).is_ok() { return Ok(3); }
  let mut empty = Bytes.new(); finish_tar(&mut empty)?;
  if empty.len() != 1024 || read_tar(&empty)?.len() != 0 { return Ok(4); }
  let mut sizes = List.new(); sizes.push(0); sizes.push(1); sizes.push(511); sizes.push(512); sizes.push(513);
  let mut i = 0;
  while i < sizes.len() {
    let n = match sizes.get(i) { Some(v) => v, None => 0 }; let data = zeros(n); let mut tar = Bytes.new();
    append_file(&mut tar, &"app", 365, &data)?; finish_tar(&mut tar)?;
    let files = read_tar(&tar)?; if files.len() != 1 { return Ok(5); }
    match files.get(0) {
      Some(f) => {
        let contents = tar_data(&f);
        if tar_name(&f) != "app" || tar_mode(&f) != 365 || contents.len() != n { return Ok(6); }
        let mut again = Bytes.new(); append_file(&mut again, &tar_name(&f), tar_mode(&f), &contents)?; finish_tar(&mut again)?;
        if digest(&again) != digest(&tar) { return Ok(7); }
      }
      None => { return Err(400); }
    }
    i = i + 1;
  }
  let mut tar = Bytes.new(); append_file(&mut tar, &"app", 365, &"abc".as_bytes())?; finish_tar(&mut tar)?;
  if tar.len() != 2048 || tar.get(100).unwrap_or(0) != 48 || tar.get(104).unwrap_or(0) != 53 || tar.get(107).unwrap_or(1) != 0 || tar.get(156).unwrap_or(0) != 48 { return Ok(8); }
  let mut offsets = List.new(); offsets.push(0); offsets.push(100); offsets.push(124); offsets.push(148); offsets.push(154); offsets.push(155); offsets.push(156); offsets.push(157); offsets.push(257); offsets.push(263); offsets.push(345); offsets.push(511); offsets.push(515); offsets.push(1024); offsets.push(2047);
  i = 0;
  while i < offsets.len() {
    let mut bad = tar.clone(); let off = match offsets.get(i) { Some(v) => v, None => 0 }; bad.set(off, 255).map_err(|e| 400)?;
    if read_tar(&bad).is_ok() { return Ok(9); } i = i + 1;
  }
  if read_tar(&tar.slice(0, 1536).map_err(|e| 400)?).is_ok() { return Ok(10); }
  let before = tar.len();
  if append_file(&mut tar, &"../app", 365, &Bytes.new()).is_ok() || append_file(&mut tar, &"/app", 365, &Bytes.new()).is_ok() || append_file(&mut tar, &"app", 511, &Bytes.new()).is_ok() || tar.len() != before { return Ok(11); }
  return Ok(0);"#);
}
#[test]
fn json_descriptor_rejects_invalid_shapes_and_references() {
    let hash = "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let valid = format!(
        r#"{{"size":3,"digest":"{hash}","mediaType":"application/vnd.oci.image.layer.v1.tar"}}"#
    );
    let invalid = [
        valid.replace("\"size\":3", "\"size\":-1"),
        valid.replace("\"size\":3", "\"size\":134217729"),
        valid.replace("\"size\":3", "\"size\":3.0"),
        valid.replace("\"size\":3", "\"size\":9223372036854775808"),
        valid.replace("\"size\":3", "\"size\":3,\"size\":3"),
        valid.replace("\"size\":3", "\"other\":3"),
        valid.replace("sha256:", "SHA256:"),
        valid.replace("ba7816", "Ba7816"),
        valid.replace("ba7816", "ba78"),
        valid.replace("v1.tar", "v1.tar+gzip"),
        "[]".into(),
        "{}".into(),
        "null".into(),
        "{\"x\":0,\"x\":1}".into(),
    ];
    let mut body = bytes("good", valid.as_bytes());
    body.push_str(
        "let d = read_descriptor(&good)?; verify_descriptor(&d, &\"abc\".as_bytes())?;\n",
    );
    for (i, bad) in invalid.iter().enumerate() {
        body.push_str(&bytes(&format!("bad{i}"), bad.as_bytes()));
        body.push_str(&format!(
            "if read_descriptor(&bad{i}).is_ok() {{ return Ok({}); }}\n",
            i + 1
        ));
    }
    body.push_str(r#"
  let wrong = descriptor(&"application/vnd.oci.image.config.v1+json", &"abc".as_bytes())?;
  if config_json(&wrong).is_ok() || manifest_json(&d, &d).is_ok() || index_json(&d).is_ok() { return Ok(90); }
  read_json(&layout_json())?;
  let cfg = config_json(&d)?; read_json(&cfg)?;
  let c = descriptor(&"application/vnd.oci.image.config.v1+json", &cfg)?;
  let m = manifest_json(&c, &d)?; read_json(&m)?;
  let md = descriptor(&"application/vnd.oci.image.manifest.v1+json", &m)?;
  read_json(&index_json(&md)?)?;
  return Ok(0);"#);
    run(&body);
}
#[test]
fn independent_python_tar_json_and_hashlib() {
    let s = source(
        "main() -> Result<String, Int>",
        r#"
  let mut layer = Bytes.new(); append_file(&mut layer, &"app", 365, &"abc".as_bytes())?; finish_tar(&mut layer)?;
  let l = descriptor(&"application/vnd.oci.image.layer.v1.tar", &layer)?;
  let cfg = config_json(&l)?; let c = descriptor(&"application/vnd.oci.image.config.v1+json", &cfg)?;
  let man = manifest_json(&c, &l)?; let m = descriptor(&"application/vnd.oci.image.manifest.v1+json", &man)?;
  let idx = index_json(&m)?;
  let mut tar = Bytes.new();
  append_file(&mut tar, &"index.json", 420, &idx)?;
  append_file(&mut tar, &"oci-layout", 420, &layout_json())?;
  let mut lp = "blobs/sha256/".as_bytes(); lp.extend(&descriptor_digest(&l).as_bytes().slice(7, 71).map_err(|e| 400)?);
  let mut cp = "blobs/sha256/".as_bytes(); cp.extend(&descriptor_digest(&c).as_bytes().slice(7, 71).map_err(|e| 400)?);
  let mut mp = "blobs/sha256/".as_bytes(); mp.extend(&descriptor_digest(&m).as_bytes().slice(7, 71).map_err(|e| 400)?);
  append_file(&mut tar, &lp.to_string_utf8().map_err(|e| 400)?, 420, &layer)?;
  append_file(&mut tar, &cp.to_string_utf8().map_err(|e| 400)?, 420, &cfg)?;
  append_file(&mut tar, &mp.to_string_utf8().map_err(|e| 400)?, 420, &man)?;
  finish_tar(&mut tar)?; read_tar(&tar)?;
  return Ok(hex(&tar));"#,
    );
    // テスト内のfixture結線だけ。製品の生成器入口・I/Oは6-2dへ残す。
    let s = s.replacen(
        "use tools.image.",
        "use tools.image.binary.hex;\nuse tools.image.",
        1,
    );
    let sources = sources(&s);
    let value = common::run_sources(&sources, "main", &[]).unwrap().value;
    let hex = value
        .strip_prefix("Ok(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or_else(|| panic!("{value}"));
    let archive: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    let dir = common::native::scratch("image-formats-external");
    let path = dir.join("fixture.tar");
    std::fs::write(&path, archive).unwrap();
    let out = std::process::Command::new("python3").arg("-c").arg(r#"
import hashlib, io, json, sys, tarfile
with tarfile.open(sys.argv[1], 'r:') as archive:
    members = archive.getmembers()
    assert len(members) == 5
    assert all(m.isfile() and m.mode == 0o644 and m.uid == m.gid == m.mtime == 0 and m.uname == m.gname == '' for m in members)
    blobs = {m.name: archive.extractfile(m).read() for m in members}
    assert json.loads(blobs['oci-layout']) == {'imageLayoutVersion': '1.0.0'}
    index = json.loads(blobs['index.json'])
    assert index['schemaVersion'] == 2 and len(index['manifests']) == 1
    md = index['manifests'][0]
    assert md['platform'] == {'architecture': 'amd64', 'os': 'linux'}
    assert md['annotations'] == {'org.opencontainers.image.ref.name': 'sigil-app:latest'}
    def ref(d, media):
        assert d['mediaType'] == 'application/vnd.oci.image.' + media
        b = blobs['blobs/sha256/' + d['digest'][7:]]
        assert d['size'] == len(b) and d['digest'] == 'sha256:' + hashlib.sha256(b).hexdigest()
        return b
    manifest = json.loads(ref(md, 'manifest.v1+json'))
    assert manifest['schemaVersion'] == 2 and manifest['mediaType'] == md['mediaType'] and len(manifest['layers']) == 1
    cfg = json.loads(ref(manifest['config'], 'config.v1+json'))
    assert cfg['architecture'] == 'amd64' and cfg['os'] == 'linux'
    assert cfg['config'] == {'User': '65532:65532', 'Entrypoint': ['/app'], 'WorkingDir': '/'}
    assert cfg['rootfs'] == {'type': 'layers', 'diff_ids': [manifest['layers'][0]['digest']]}
    assert 'created' not in cfg and 'history' not in cfg
    layer = ref(manifest['layers'][0], 'layer.v1.tar')
    with tarfile.open(fileobj=io.BytesIO(layer), mode='r:') as inner:
        files = inner.getmembers()
        assert len(files) == 1
        app = files[0]
        assert app.name == 'app' and app.mode == 0o555 and app.uid == app.gid == app.mtime == 0
        assert app.uname == app.gname == '' and app.isfile() and inner.extractfile(app).read() == b'abc'
    for name, data in blobs.items():
        if name.endswith('.json') or name == 'oci-layout' or data.startswith(b'{'):
            # 正規形: 空白・改行なし、UTF-8、JSONの固定順を保つ。
            assert json.dumps(json.loads(data), ensure_ascii=False, separators=(',', ':')).encode() == data
print('tarfile/json/hashlib: OK')
"#).arg(&path).output().expect("独立した外部互換性検査にはpython3が必要");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "tarfile/json/hashlib: OK"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
