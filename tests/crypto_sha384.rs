//! ADR 0058 段 2b: SHA-384（`sha384` と `Sha384`）の利用者向けテスト。
//!
//! 出典と検算（ADR 0058 決定 4）:
//! - RFC 6234 §8.5 のテスト: `abc`（TEST1）、112 バイトの `abcdefghbcdefghi…`（TEST2 の 2 つ目）。
//!   値は NIST CAVP の SHA-384 と、Python の `hashlib`（OpenSSL）で独立に再計算して一致を確認した。
//!   RFC 6234 の TEST3（`a` を 100 万個）・TEST4・TEST10 は、評価器で数時間規模になるため含めない。
//! - NIST CAVP `SHA384ShortMsg.rsp`（CAVS 11.0）から、パディングとブロックの境界（0・1・2・3・55・56・63・64・65・
//!   111・112・113・119・120・127・128 バイト）を選んだ。`SHA384LongMsg.rsp` から 3 件（複数ブロック）。
//!   入手: csrc.nist.gov へはこの実行環境から到達できないため、pyca/cryptography の `vectors/` にある CAVP の写し
//!   （`hashes/SHA2/SHA384ShortMsg.rsp` の SHA-256 `7ea7bcf00fadc20949fae63703e40681ddf288fea808471cb3cbc95f3ec16811`、`SHA384LongMsg.rsp` の SHA-256 `536171765a4278c000ac3c9913edb2eed0ca7ccd5a10b72ed79fdfe7901a6d6a`）から取得した。
//!   選んだ全件を `hashlib` で再計算して一致を確認している。
//! - ワンショット `sha384(data)` とストリーミング `Sha384`（new / update / finalize）の両方を確かめる。ストリーミングは、
//!   分割（1 バイトずつ、ブロック境界の前後）によらずワンショットと一致する（ADR 0024 決定 4）。
//! - ここでのテストは評価器とネイティブの両方で回る（`common::run` が突き合わせる）。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module crypto_sha384.t\n  targets server\n  trust none\n\n";

const MISMATCH_FN: &str = "fn mismatch(d: &Bytes, want: &List<Int>) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut bad = 0;\n  let mut i = 0;\n  while i < want.len() {\n    let w = match want.get(i) { Some(v) => v, None => 0 };\n    let g = d.get(i).unwrap_or(0 - 1);\n    if g != w {\n      bad = bad + 1;\n    }\n    i = i + 1;\n  }\n  if d.byte_len() != want.len() {\n    bad = bad + 100;\n  }\n  return bad;\n}\n\n";

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

fn build_bytes(name: &str, bytes: &[u8]) -> String {
    let mut s = format!("  let mut {name} = Bytes.new();\n");
    for b in bytes {
        s.push_str(&format!("  {name}.push({b});\n"));
    }
    s
}

fn build_expected(name: &str, hex: &str) -> String {
    let mut s = format!("  let mut {name} = List.new();\n");
    for b in unhex(hex) {
        s.push_str(&format!("  {name}.push({b});\n"));
    }
    s
}

fn run_oneshot(input: &[u8], expected_hex: &str) -> i64 {
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  let d = sha384(&input);\n{}  return mismatch(&d, &want);\n}}\n",
        build_bytes("input", input),
        build_expected("want", expected_hex),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

/// 入力を `chunks` の大きさで順に update し、finalize の結果を期待値と比べる。
fn run_streaming(input: &[u8], chunks: &[usize], expected_hex: &str) -> i64 {
    assert_eq!(chunks.iter().sum::<usize>(), input.len());
    let mut body = String::new();
    body.push_str("  let mut h = Sha384.new();\n");
    let mut pos = 0;
    for (ci, &n) in chunks.iter().enumerate() {
        body.push_str(&build_bytes(&format!("c{ci}"), &input[pos..pos + n]));
        body.push_str(&format!("  h.update(&c{ci});\n"));
        pos += n;
    }
    body.push_str("  let d = h.finalize();\n");
    body.push_str(&build_expected("want", expected_hex));
    body.push_str("  return mismatch(&d, &want);\n");
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}}}\n"
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

const EMPTY_HEX: &str = "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b";
const ABC_HEX: &str = "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7";
const MSG112: &[u8] = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";
const MSG112_HEX: &str = "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039";

/// (バイト長, メッセージ, ダイジェスト)。NIST CAVP SHA384ShortMsg.rsp から選んだ。
const CAVS_SHORT: &[(usize, &str, &str)] = &[
    (0, "", "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"),
    (1, "c5", "b52b72da75d0666379e20f9b4a79c33a329a01f06a2fb7865c9062a28c1de860ba432edfd86b4cb1cb8a75b46076e3b1"),
    (2, "6ece", "53d4773da50d8be4145d8f3a7098ff3691a554a29ae6f652cc7121eb8bc96fd2210e06ae2fa2a36c4b3b3497341e70f0"),
    (3, "1fa4d5", "e4ca4663dff189541cd026dcc056626419028774666f5b379b99f4887c7237bdbd3bea46d5388be0efc2d4b7989ab2c4"),
    (55, "a8de55170c6dc0d80de32f508bf49b7046aeea4b975a511b5ea3dc853bfa4b1e0138202d6785f6b3017914a86f824288d586dc0e8bc924", "2bc3b10c148200f7919b57afe1d7db773ffd235e04fec6897dd94f13ad9c437ef50900a40937f82a39daf2aa2b3dfd68"),
    (56, "accd9d05fb7ef3043470836137554af117440b3ccca7a280285494f90dfaea60dcbf40b230271932cd3875b1d3dca60d38865ff874180efa", "b9e02df93e9254180d6a15288d77088b5a5ce517644109b4e2532be315f08dee43491608a1127dcdf69397406e23d231"),
    (63, "0b94a0f43a92408963a59ded01a9338283a6ff1bfbacd9051a0104445c7f041e8037afde3b5a87d22d5a4796144cbc944091d6cc47b5ffd1f997ab1496ca31", "07a045c9590e9901d0598e604c4649554a823de996fa438cc81a634344eeb98e5f3c0c234ba30e2285a4d7ab568f2610"),
    (64, "93035d3a13ae1b06dd033e764aca0124961da79c366c6c756bc4bcc11850a3a8d120854f34290fff7c8d6d83531dbdd1e81cc4ed4246e00bd4113ef451334daa", "8d46cc84b6c2deb206aa5c861798798751a26ee74b1daf3a557c41aebd65adc027559f7cd92b255b374c83bd55568b45"),
    (65, "bfb94dfbe0d9a509b78d164a722050054dad91c9a8e260545d037eb450321aac48ed4459fdd8a41572bd6c9c84d18b6ec339247482cc3ee52a1bbd6bd4ae918216", "13af0be02986ea3176e8c65534ec9f32c23b53c93a73b15c26b9ecbd8a1181ae184a372e9f5e0596cd6606849aeae8e0"),
    (111, "a04f390a9cc2effad05db80d9076a8d4b6cc8bba97b27b423670b290b8e69c2b187230011c1481ac88d090f39154659494db5e410851c6e8b2b8a93717cae76037e0881978124fe7e1a0929d8891491f4e99646cc94062dc82411fa66130eda46560e75b98048236439465125e737b", "e7089d72945cef851e689b4409cfb63d135f0b5cdfb0dac6c3a292dd70371ab4b79da1997d7992906ac7213502662920"),
    (112, "f419494c3c6d0727b3395a483a2167182a7252f4fd099c2d4b71b053f94bb8b3adf3b51e8460cfec084ce9415c95798fbae4975c208c544645b54c44d2b97f2ecfce5c805be61f5ba1d35dcc07afdd51a87baa990506668cf710e18be9b0ebf943f366fa29c69f7a6616de72a3353b66", "aead8688c58c6ba4e9cadb4756b465dce0fb06f1cfaa478197f2ea89414e47e9572034adfed160703c79b82b3fd7ab78"),
    (113, "aaf7584d53006cbf2d2040e51b7feebd2bbf1e9f6d817cd8062a6a9680e7f10464eefeb50b07cb46b14b9b3fcb2caa3b9ab664490115d5919456613bf172b58c5388fd52646a5783535b88212717ef605314b70b8a085024d4ab1fcbe2be74609e4cbdec0730fabd3cd77151d647a3767b", "e6e79d8c61d0ea9fc70dd4dec1fa432849c3396e717b170badbd87a4c7974efc590ab8c1183a6232beff14534f004b02"),
    (119, "a5c4a47a04f4714269d5d922ba4694060aa2df49193720c819fac93bb8787ec55a107ac9a6602f0045fd2cc8e66744bf863ced91eeabe60e7d2c1d80276ecf3bbe91f1757096cf589214f3569c2c48bd74be7f8befddb28395814780a47c180a58b0d0276a7e9873d682f473e27de7275c925ede23b6cc", "6bd9e1303579d815f58e8c6c9855850133251778a632f7b312c4b3163b29b5ef6cb9511a08a31a237d9a7604afbfa056"),
    (120, "75264774af69ec7ee3125e205bd6d1cb8fdb22f7ea6dbe72d1f2c0f7e2205902796d75e379c79b11498615c21c9f52b8761a885eecc69d132b2b48c63bc074c3055ee5cc13f51d6c987e8188b030b837e8f754d40122b451f15b28cd2bdd576920e1de5806593a36d8e1e89b9ef3caefee5acd80b3e9c9d1", "ffa9e4e856d06227c1ccb959be558309cc10633173f4b66ceb382923b52b9150acfb08a7393500477a6ee4425a827e76"),
    (127, "dbed7612448d46cbe0a384d1c93233f02ffd1c984ba765299518656d3723b766c1658d4b1e7047cdc729459e366ef9349efc40cbd990f2a9a24db7a5045e1dea12dce8f9d9f2aaed933f93031e7b8959ac5e7bf6bbbdf30b48f7eb783f8fe292371a2f245c5c94b4acae160767a20ce7c0ea7723d97691d8eedda9efd1fe2d", "fb531a1ed181c732311e56f4b56ed91dcacc0dd6bf1eb4a44be6f87dd7cb1ef9dfb0310f4a79eaaa3f32bf3914d8624e"),
    (128, "3bf52cc5ee86b9a0190f390a5c0366a560b557000dbe5115fd9ee11630a62769011575f15881198f227876e8fe685a6939bc8b89fd48a34ec5e71e131462b2886794dffa68ccc6d564733e67ffef25e627c6f4b5460796e3bce67bf58ca6e8e555bc916a8531697ac948b90dc8616f25101db90b50c3d3dbc9e21e42ff387187", "12b6cb35eda92ee37356ddee77781a17b3d90e563824a984faffc6fdd1693bd7626039635563cfc3b9a2b00f9c65eefd"),
];

/// (バイト長, メッセージ, ダイジェスト)。NIST CAVP SHA384LongMsg.rsp から選んだ。
const CAVS_LONG: &[(usize, &str, &str)] = &[
    (227, "62c6a169b9be02b3d7b471a964fc0bcc72b480d26aecb2ed460b7f50016ddaf04c51218783f3aadfdff5a04ded030d7b3fb7376b61ba30b90e2da921a4470740d63fb99fa16cc8ed81abaf8ce4016e50df81da832070372c24a80890aa3a26fa675710b8fb718266249d496f313c55d0bada101f8f56eeccee4345a8f98f60a36662cfda794900d12f9414fcbdfdeb85388a814996b47e24d5c8086e7a8edcc53d299d0d033e6bb60c58b83d6e8b57f6c258d6081dd10eb942fdf8ec157ec3e75371235a8196eb9d22b1de3a2d30c2abbe0db7650cf6c7159bacbe29b3a93c92100508", "0730e184e7795575569f87030260bb8e54498e0e5d096b18285e988d245b6f3486d1f2447d5f85bcbe59d5689fc49425"),
    (326, "ffd67890ff77f344ad4f067df2f4ff1db8f541c7a2bd9ae9fabad0faebbf7d00f0a71d568c3c66ac3c57d84faa4894ab2377710e4b4c4dae0f4da1eedc8658dd0e2ee2ffac8784515206f2876eb41f98afd4547cbbc68034212bcf0c8e4a7d1d43b3ed15c621f53bd8a57cada80148ec4652119b5af3da84169d81dc69d394c8767d662044d36272b77ca04abff7b6b0cf3bd1f3919a04a5d8ebdfe7d6e844e78fd576a68d6373ffd5d384e51b5e12ec32d5bb0ac685a59f4d5d12b43b533580750345310999cfe91cf2500624fe03a65769f86a627a667b5f3b42cb01da109e124ffa48203f1f3873202d35429f32e8263eaf9bce42ef40f5cc96b591467d464d00bd743a1b0af4c1a743fbdd0846b9879e092371a5e7f6f65937f9515e23820e60b83bbff73926f0cdb9df5d02e82262cf2e8cb26af6a64c2a4d1fabecab593db510281799", "1396fea95ce0c1c1c224b50a07dd7197f1d62b993c7fe9e1cc1a56101920d4b0fecef587fbcd56b854c8c9da95132f02"),
    (425, "be105a6c585a766aa1f290b632219ff864ea6674b5e3f9846d447d46813e2f92b78ea82f0b515f46511a6f161d429aeae07f8e4853b8686c191877f5a06242dd4884ccaec76e16f3cc24e6ede212c78897a1518dde07aa19b3634d4efd090a48b81a4e535359a55b57397da44a3b2dbbd37636ac1f77c875824c88bb62dc90bc517ab78579b913643d81bda11d62d469da29c50bdbb1c967d0fcafa29582db1f59c98fabff3669f8c4232d4d2332c57bf6e08613dd5db5d6e39b4a6d5fa4f35b19325c2fae79aefe36485610235007da6cc3022cec2295ac0550e18388eae156d9dba8784e2aea5ed917be53e767a26c87fcc0bfcfa870d07b43fd4cd8fbacfae1ecfaeea7f12600f6b9ef7c351d9f1b8eb048324f984e2a909d253017805c2d788edbf9807469ccd45571f5d61a05cec80a23ef349a37a28b86e2970c20fad9e7e8d201a35ebbaabd14ca249207dc6d7e2fd85c46553420b325bbe980848de57724676ed6552482f71e8473308c2ddff94aefe34c724c8c52a3388e3b541d396d6722a8e201ad3ccb9a26497a50ff0e7e81f1eb109888ceae27e1ef0537b3bd14dc8c178f0c5dc081", "39cf9206d78cfb437000b100661753181b5f262b539c208b442e7a022cd3234c58751621ff0ee9e9d3607174e36aaa88"),
];

#[test]
fn sha384_rfc6234_empty() {
    assert_eq!(run_oneshot(b"", EMPTY_HEX), 0);
}

#[test]
fn sha384_rfc6234_abc() {
    assert_eq!(run_oneshot(b"abc", ABC_HEX), 0);
}

#[test]
fn sha384_rfc6234_112_bytes_padding_spills_to_second_block() {
    // 112 バイト: 0x80 と 128 ビットの長さ欄が 2 ブロック目へ広がる最小の長さ。
    assert_eq!(run_oneshot(MSG112, MSG112_HEX), 0);
}

#[test]
fn sha384_nist_cavp_short_msg_boundaries() {
    for (n, msg, md) in CAVS_SHORT {
        let m = unhex(msg);
        assert_eq!(m.len(), *n);
        assert_eq!(run_oneshot(&m, md), 0, "ShortMsg {n} バイト");
    }
}

#[test]
fn sha384_nist_cavp_long_msg_multi_block() {
    for (n, msg, md) in CAVS_LONG {
        let m = unhex(msg);
        assert_eq!(m.len(), *n);
        assert_eq!(run_oneshot(&m, md), 0, "LongMsg {n} バイト");
    }
}

#[test]
fn sha384_streaming_one_byte_at_a_time() {
    let (n, msg, md) = CAVS_LONG[0];
    let m = unhex(msg);
    assert_eq!(m.len(), n);
    let chunks = vec![1usize; m.len()];
    assert_eq!(run_streaming(&m, &chunks, md), 0);
}

#[test]
fn sha384_streaming_block_edge_splits() {
    // 128 バイトのブロック境界の前後で切る（127+1、128、129、境界ちょうどの更新）。
    let (n, msg, md) = CAVS_LONG[1];
    let m = unhex(msg);
    assert_eq!(m.len(), n);
    for first in [1usize, 127, 128, 129, 200] {
        let rest = m.len() - first;
        assert_eq!(run_streaming(&m, &[first, rest], md), 0, "first={first}");
    }
    assert_eq!(run_streaming(&m, &[128, 128, m.len() - 256], md), 0);
}

#[test]
fn sha384_streaming_empty_and_single_update() {
    let want = build_expected("want", EMPTY_HEX);
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut h = Sha384.new();\n  let d0 = h.finalize();\n{want}  let a = mismatch(&d0, &want);\n  let mut h2 = Sha384.new();\n  let e = Bytes.new();\n  h2.update(&e);\n  let d1 = h2.finalize();\n  return a * 1000 + mismatch(&d1, &want);\n}}\n"
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    assert_eq!(r.value, "0");
}
