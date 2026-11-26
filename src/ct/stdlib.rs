//! 標準ライブラリの定数時間の対象表（ADR 0024 決定 3）。
//!
//! `sigil test --ct` が、利用者のモジュールから標準ライブラリの定数時間の関数を呼ぶ形を
//! 許す（R4 の判定）ために使う。標準ライブラリ自身の検査は `tests/ct_rules.rs` が行う。

/// 定数時間であるべき関数の表。(関数名, 公開の引数名の一覧)。
/// ここにない引数はすべて秘密として扱う。
pub const STDLIB_TARGETS: &[(&str, &[&str])] = &[
    // 既にある実装（ADR 0023）。引数 a・b の中身が秘密、長さは公開。
    ("std.core.bytes.ct_equal", &[]),
    // `Secret` の入口（PR 6-1b）。メソッドの SIR 名は裸の名前になる。
    // `from_bytes` の引数は取り込む秘密、`expose` は明示的な取り出し
    // （秘密を返すこと自体は違反ではない）、`byte_len` は長さ（公開）だけを見る。
    ("from_bytes", &[]),
    ("expose", &[]),
    ("byte_len", &[]),
    // SHA-256（PR 6-1c で実装）。ワンショット `sha256` の引数 `data` の
    // 中身は秘密、長さは公開（解析のモデリングで BytesLen は公開になる）。
    // 圧縮 `sha256_compress` は、公開なのはブロックの数 `block_count`
    // だけ（ループ回数・添字はそこから派生する。ADR 0024 決定 3 の表）。
    ("std.crypto.sha256", &[]),
    ("std.crypto.sha256_compress", &["block_count"]),
    // HMAC と PBKDF2（ADR 0024 決定 1 の 4・5、PR 6-1d で実装）。
    // なお std.crypto の比較は言語の自由関数 `ct_eq` が担い（ADR 0024 決定 1）、
    // 実体は上の `std.core.bytes.ct_equal` なので、ここには別に載せない。
    ("std.crypto.hmac_sha256", &[]),
    // PBKDF2 が使う内部の補助（鍵のパッドを処理済みの状態から HMAC を求める。PR 6-1e）。
    // 引数の状態・メッセージはすべて秘密、長さは公開。
    ("std.crypto.hmac_from_pads", &[]),
    ("std.crypto.verify_hmac_sha256", &[]),
    // HKDF-SHA-256（ADR 0058 決定 3、段 1a）。salt・IKM・PRK・出力が秘密、
    // info と out_len は公開。内部の `hmac_any_key` は任意長の鍵による
    // HMAC で、鍵・メッセージとも秘密、長さは公開。
    ("std.crypto.hmac_any_key", &[]),
    ("std.crypto.hkdf_extract_sha256", &[]),
    ("std.crypto.hkdf_expand_sha256", &["info", "out_len"]),
    // SHA-384 と HKDF-SHA-384（ADR 0058 決定 3、段 2b）。`sha384` の入力、HMAC の鍵・メッセージ、
    // salt・IKM・PRK・出力は秘密、info と out_len・各長さは公開。圧縮 `sha512_compress` の公開は
    // ブロックの数 `block_count` だけ、語の読み `sha512_word` の公開は位置 `off` だけ（ラウンド番号と
    // ブロック位置から決まる）。64 ビット語の加算 `sha512_add5` は引数をすべて秘密として検査する。
    ("std.crypto.sha384", &[]),
    ("std.crypto.sha512_compress", &["block_count"]),
    ("std.crypto.sha512_add5", &[]),
    ("std.crypto.sha512_word", &["off"]),
    ("std.crypto.hmac_any_key_sha384", &[]),
    ("std.crypto.hkdf_extract_sha384", &[]),
    ("std.crypto.hkdf_expand_sha384", &["info", "out_len"]),
    // AES-128-GCM（ADR 0058 決定 3、段 1b）。鍵・平文・鍵展開・GHASH の状態・認証タグの
    // 計算途中の値は秘密。公開は nonce・AAD・受信した暗号文とタグ・データの長さ・
    // 固定の処理回数。S-box と GHASH は表引きをせず、固定した演算列でマスクを使う。
    // AES の内部部品は、引数をすべて秘密として検査する（ラウンド数・ブロック数は
    // 定数と長さから決まり、引数には現れない）。
    ("std.crypto.aes_xtime", &[]),
    ("std.crypto.aes_gmul", &[]),
    ("std.crypto.aes_sbox", &[]),
    ("std.crypto.aes_expand_key", &[]),
    ("std.crypto.aes_encrypt_block", &[]),
    ("std.crypto.ghash_mul", &[]),
    ("std.crypto.ghash_words", &["off"]),
    ("std.crypto.ghash_step", &[]),
    ("std.crypto.ghash", &["aad", "ct"]),
    ("std.crypto.gcm_ctr_xor", &["nonce"]),
    ("std.crypto.gcm_tag", &["nonce", "aad", "ct"]),
    ("std.crypto.aes128_gcm_tag_matches", &["got"]),
    ("std.crypto.aes128_gcm_seal", &["nonce", "aad"]),
    ("std.crypto.aes128_gcm_open", &["nonce", "aad", "sealed"]),
    // AES-256-GCM（ADR 0058 決定 3、段 2c）。鍵展開・ブロック暗号化・GHASH は AES-128-GCM と
    // 共通の関数（鍵長はラウンド鍵の長さで決まり、秘密に依存しない）。公開は nonce・AAD・
    // 受信した暗号文とタグ・データの長さ。
    ("std.crypto.aes256_gcm_tag_matches", &["got"]),
    ("std.crypto.aes256_gcm_seal", &["nonce", "aad"]),
    ("std.crypto.aes256_gcm_open", &["nonce", "aad", "sealed"]),
    // ChaCha20-Poly1305（ADR 0058 決定 3、段 2a）。鍵・一回限りの Poly1305 鍵・キーストリーム・
    // Poly1305 の作業領域（h・r・s）・平文は秘密。公開は nonce・AAD・受信した暗号文とタグ・メッセージ・
    // データの長さ・ブロックの位置と固定の処理回数。ChaCha20 は加算・排他的論理和・回転だけ、
    // Poly1305 は 26 ビット 5 語の積和とマスクによる選択だけで、秘密による分岐・添字・除算を持たない。
    ("std.crypto.chacha_le32", &["off"]),
    ("std.crypto.chacha20_block", &["nonce", "counter"]),
    ("std.crypto.chacha20_xor", &["nonce", "counter"]),
    ("std.crypto.poly_pack", &[]),
    ("std.crypto.poly_read", &["i"]),
    ("std.crypto.poly1305_step", &["blk", "off", "hibit"]),
    ("std.crypto.poly1305_mac", &["msg"]),
    ("std.crypto.chacha20_poly1305_tag", &["nonce", "aad", "ct"]),
    ("std.crypto.chacha20_poly1305_tag_matches", &["got"]),
    ("std.crypto.chacha20_poly1305_seal", &["nonce", "aad"]),
    (
        "std.crypto.chacha20_poly1305_open",
        &["nonce", "aad", "sealed"],
    ),
    ("from_secret", &[]),
    ("from_slice", &["b"]),
    // X25519（ADR 0058 決定 3、段 1c）。スカラー、体演算の中間値、
    // 共有値は秘密。相手の公開鍵、演算回数と添字は公開。
    ("std.crypto.x25519_public_key", &[]),
    ("std.crypto.x25519_shared_secret", &["peer"]),
    ("std.crypto.x25519_scalar_mult", &["u"]),
    ("std.crypto.x25519_fe_decode", &[]),
    ("std.crypto.x25519_fe_encode", &[]),
    ("std.crypto.x25519_fe_read", &["i"]),
    ("std.crypto.x25519_fe_pack", &[]),
    ("std.crypto.x25519_fe_reduce", &[]),
    ("std.crypto.x25519_fe_add", &[]),
    ("std.crypto.x25519_fe_sub", &[]),
    ("std.crypto.x25519_fe_mul", &[]),
    ("std.crypto.x25519_fe_square", &[]),
    ("std.crypto.x25519_fe_square_n", &["count"]),
    ("std.crypto.x25519_fe_invert", &[]),
    ("std.crypto.x25519_fe_swap", &[]),
    ("std.crypto.x25519_is_zero", &[]),
    // ECDSA P-256（ADR 0064 決定 2）。秘密鍵・nonce・HMAC の状態・体演算と点演算の中間値・署名確定前の
    // r と s は秘密。公開は署名対象のメッセージ、公開鍵、署名、DER の入出力、指数 p-2・n-2、反復の添字。
    // 署名で公開できるのは最終の署名と成否（`p256_accepted`・`p256_scalar_ok` の結果）だけ。
    ("std.crypto.p256_fe_read", &["i"]),
    ("std.crypto.p256_fe_pack", &[]),
    ("std.crypto.p256_fe_decode", &[]),
    ("std.crypto.p256_fe_encode", &[]),
    ("std.crypto.p256_fe_select", &[]),
    ("std.crypto.p256_fp_mul", &[]),
    ("std.crypto.p256_fp_add", &[]),
    ("std.crypto.p256_fp_sub", &[]),
    ("std.crypto.p256_fn_mul", &[]),
    ("std.crypto.p256_fn_add", &[]),
    ("std.crypto.p256_const_one_p", &[]),
    ("std.crypto.p256_const_zero", &[]),
    ("std.crypto.p256_const_b", &[]),
    ("std.crypto.p256_const_r2_p", &[]),
    ("std.crypto.p256_const_r2_n", &[]),
    ("std.crypto.p256_const_one_n", &[]),
    ("std.crypto.p256_const_raw_one", &[]),
    ("std.crypto.p256_const_gx", &[]),
    ("std.crypto.p256_const_gy", &[]),
    ("std.crypto.p256_const_n", &[]),
    ("std.crypto.p256_const_p", &[]),
    ("std.crypto.p256_const_pm2", &[]),
    ("std.crypto.p256_const_nm2", &[]),
    ("std.crypto.p256_fp_pow", &["e"]),
    ("std.crypto.p256_fn_pow", &["e"]),
    ("std.crypto.p256_pt_add", &[]),
    ("std.crypto.p256_pt_dbl", &[]),
    ("std.crypto.p256_pt_select", &[]),
    ("std.crypto.p256_pt_identity", &[]),
    ("std.crypto.p256_pt_base", &[]),
    ("std.crypto.p256_scalar_mult", &[]),
    ("std.crypto.p256_lt", &[]),
    ("std.crypto.p256_nonzero", &[]),
    ("std.crypto.p256_scalar_ok", &[]),
    ("std.crypto.p256_accepted", &[]),
    ("std.crypto.p256_affine_x", &[]),
    ("std.crypto.p256_affine_y", &[]),
    ("std.crypto.p256_finish", &[]),
    ("std.crypto.p256_candidate", &[]),
    ("std.crypto.p256_pick", &[]),
    ("std.crypto.p256_signature_from", &[]),
    ("std.crypto.p256_drbg_init", &[]),
    ("std.crypto.p256_drbg_next", &[]),
    ("std.crypto.p256_drbg_step", &[]),
    ("std.crypto.ecdsa_p256_sign", &["message"]),
    ("std.crypto.p256_point_on_curve", &["x", "y"]),
    ("std.crypto.p256_slice32", &["data", "start"]),
    ("std.crypto.p256_public_valid", &["encoded"]),
    ("std.crypto.p256_public_from_bytes", &["encoded"]),
    ("std.crypto.p256_ecdh_public_from_bytes", &["encoded"]),
    ("std.crypto.p256_public_bytes", &[]),
    ("std.crypto.ecdsa_p256_public_key", &[]),
    // P-256 の ECDH（ADR 0065 決定 3）。秘密のスカラー、共有点の全ての中間値、共有値は秘密。
    // 公開は受け取る点（公開鍵）、公開鍵の出力、受理結果。
    ("std.crypto.ecdh_p256_public_key", &[]),
    ("std.crypto.ecdh_p256_shared_secret", &["peer"]),
    ("std.crypto.p256_der_int", &["value"]),
    ("std.crypto.p256_der_fail", &[]),
    ("std.crypto.p256_der_read", &["data", "off"]),
    ("std.crypto.p256_signature_from_der", &["encoded"]),
    ("std.crypto.p256_signature_der", &["r", "s"]),
    (
        "std.crypto.ecdsa_p256_verify",
        &["public", "message", "signature"],
    ),
    ("to_bytes", &[]),
    ("to_der", &[]),
    ("from_der", &[]),
    ("as_bytes", &[]),
    ("is_invalid_key_length", &[]),
    ("is_all_zero_shared_secret", &[]),
    // 塩・回数・出力の長さは公開（ADR 0020 決定 1 の対象表）。
    (
        "std.crypto.pbkdf2_sha256",
        &["salt", "iterations", "dk_len"],
    ),
    // ADR 0034: ClientProof の XOR と ServerSignature 比較、および
    // 証明の base64 は秘密依存の表引き・分岐を行わない。
    ("std.pg_scram.ct_base64_char", &[]),
    ("std.pg_scram.ct_base64", &[]),
    ("std.pg_scram.ct_proof", &[]),
    ("std.pg_scram.ct_signature", &[]),
    ("std.pg_scram.respond", &["nonce", "server_first"]),
    ("std.pg_scram.respond_text", &["nonce", "server_first"]),
    (
        "std.pg_scram.respond_bare",
        &["nonce", "server_first", "bare"],
    ),
    ("std.pg_scram.verify", &["server_final"]),
    // ADR 0037: 鍵だけが秘密。署名対象・受信タグ・設定 audience・時刻は公開。
    ("std.web.ct_bearer_signature", &["message", "tag"]),
    // 検証器全体は CT と称さない。公開入力の分岐と、以下で明示した
    // 比較結果の公開後の分岐を許し、鍵の流れには R1〜R6 を適用する。
    ("std.web.verify_bearer", &["audience", "token", "now"]),
];
