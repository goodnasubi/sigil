# `src/check.rs` 分割計画（草案 v0.1）

目的: 並行開発のため、5,600 行ある型検査の核 [check.rs](../src/check.rs) を、担当レーンごとのファイルに分ける。**動作は一切変えない**（純粋な移動）。
運用ルールは [PARALLEL_DEV.md](PARALLEL_DEV.md) を参照。

## 1. 現状（`src/check.rs`、約 7,100 行）

| 行（概数） | 内容 |
|---|---|
| 1–260 | 公開 API（`check`・`typed`・`closure_captures`）、`Ty`・`CapMode`・`Resolved` などの型 |
| 260–600 | 内部の型（`TypeDef`・`Sig`・`TraitDef`・`Eff`・`Local`・`Ck` 本体など） |
| 600–1240 | `analyze`、補助関数（`rename_names*`・`collect_calls*`・`substitute` など） |
| **1240–6518** | **`impl Ck`（約 5,300 行）**。分割の主対象 |
| 6518–6640 | 自由関数（`strip_refs`・`place_path`・`is_literal` など） |
| 6643–7100 | テスト（`typed_tests`・`std_heads_tests`） |

`Ck` のフィールドはモジュール内の非公開で、子モジュールからは参照できる（Rust は子から祖先の非公開項目を参照できる）。そのため **`impl Ck` を子モジュールに分けて書ける**。

## 2. 分割方針

- `src/check.rs` を `src/check/mod.rs` に移し（履歴を保つため `git mv`）、子モジュールを `src/check/*.rs` に作る。
- `Ck` 本体・フィールド・公開 API は `mod.rs` に残す。
- 移すメソッドは、子モジュール内で `impl Ck { ... }` と書き、他のモジュールから呼ばれるものは `pub(super)` にする（機械的な変更）。
- 公開 API（`crate::check::…`）のパスは変えない。必要なら `mod.rs` で `pub use` する。
- **メソッドの本体は一文字も変えない**（可視性の修飾子と `use` の追加だけ）。

## 3. 分割先

行番号は現状の `impl Ck` 内のメソッド位置（概数）。

| 新ファイル | レーン | 移すもの |
|---|---|---|
| `check/types.rs` | B1 | `Ty`・`show`・`TypeDef`・`Sig`・`substitute`・`head_of`・`match_pattern`・`contains_param` |
| `check/unify.rs` | B1 | `fresh*`・`shallow`・`zonk`・`occurs`・`unify*`・`try_unify`・`coerce`・`refinement_base`・`val`・`holds`（1329–1635） |
| `check/generics.rs` | B1 | `instantiate`・`fresh_named`・`generics`（1635–1742） |
| `check/resolve.rs` | B1 | `resolve`・`contains_bad_base`・`well_founded`・`reaches`・`lookup`・`peel`・`type_path`（2849–3046） |
| `check/register.rs` | B2 | `register_*`・`signature*`・`std_*`・`raw_allowed`・`os_*`・`prelude`（1302–1329、2562–2849） |
| `check/traits.rs` | B2 | `register_trait`・`register_impl_for`・`check_impl_conformance`・`check_fn_conformance`・`require_traits`・`trait_satisfied`・`trait_*_sig`・`trait_qualified_call`（2065–2447、5913–6218） |
| `check/items.rs` | B2 | `run`（アイテム全体の検査の入口、1742–1990）・`poly_check`・`build_scc`（2447–2562） |
| `check/effects.rs` | B5 | `effect_terms`・`begin`・`add_eff`・`has_var`・`finish`・`match_effects`・`fn_effects`（2669–2708、3046–3232、3354–3406、6337–6364） |
| `check/fn_check.rs` | B5 | `check_refinement`・`check_fn*`・`record_report`・`body_once`（3232–3354、3406–3469） |
| `check/ownership.rs` | B3 | `use_place`・`touch`・`note_capture`・`check_holder`・`log_acc`・`check_overlap`・`borrow_srcs`・`set_res`・`adopt`・`record_move`・`consume`・`reinit`・`return_*`・`pop_scope` など（3469–3969、4278–4369） |
| `check/stmt.rs` | B4 | `stmt`・`if_stmt`・`block`・`ens_check`・`declare`・`bind`・`shadow_check`・`forget`（3969–4278、4353–4455） |
| `check/pattern.rs` | B4 | `variant_of`・`pat*`・`match_moves`・`scrutinee`・`to_p`・`enum_shapes`・`unreachable_arms`・`exhaustive`（4455–4745） |
| `check/expr.rs` | B4 | `expr*`・`name*`・`unary`・`binary`・`field`・`struct_lit`・`try_op`・`variant_value`（4796–5009、5224–5645、6454–6518） |
| `check/closure.rs` | B4 | `closure`・`apply_captures`・`closure_captures` と `CapMode`・`Capture`・`ClosureInfo`（5009–5224、1–260 の該当部分） |
| `check/call.rs` | B4 | `call`・`apply`・`args`・`method_tail`・`clone_sig`・`sig_check`（5645–5913、6218–6337、6364–6454） |
| `check/util.rs` | 共有 | 自由関数（`rename_names*`・`collect_calls*`・`conjuncts`・`strip_*`・`place_*`・`is_literal`・`op_text`・`block_returns`） |
| `check/tests.rs` | 共有 | `typed_tests`・`std_heads_tests` |

> [!NOTE]
> 上の行範囲は目安です。実際には、メソッド間の依存（例: `expr` が `call` を呼ぶ）を `pub(super)` でつなぐだけで足ります。**循環する呼び出しは Rust では問題になりません**（同じ型への `impl` のため）。

## 4. 手順（凍結期間あり）

> [!WARNING]
> 分割中は `check.rs` を触る他の PR と必ず衝突します。**凍結期間を設けて、レーン B（`check`・`borrow` を触る作業）の新規着手を止めます。** 目標は 1〜2 日。

| 段階 | 内容 | 担当 |
|---|---|---|
| S0 | 凍結の宣言。進行中の `check.rs` を触る PR を先にマージするか破棄する。ベースラインとして `cargo test` の結果と、`sigil check`・`sigil spec --json` を全サンプルに対して実行した出力を保存する | 人間または調整役 1 体 |
| S1 | `git mv src/check.rs src/check/mod.rs` だけの PR（内容変更なし）。マージ | 担当 1 体 |
| S2 | `tests.rs`・`util.rs`・`types.rs` の抽出（`impl` を含まない部分） | 同じ担当 |
| S3 | `impl Ck` を、小さい順（`closure` → `pattern` → `traits` …）に **1 ファイル 1 PR** で移す。各 PR で `cargo test` と §5 の検証を通す | 同じ担当（直列） |
| S4 | `mod.rs` に残る行数を確認（目標は 1,000 行以下）。凍結を解除する | 同じ担当 |

- **分割は 1 体が直列で行う**。並列にすると、同じファイルの削除範囲が衝突して意味がない。
- 他のレーン（A・C・D・E）は凍結の影響を受けない。この期間も並行して動かせる。

## 5. 「動作が変わっていない」ことの検証

各 PR で次を確認する。

1. `cargo fmt --check`・`cargo clippy --all-targets -- -D warnings`・`cargo test`（既存のテスト全部）が通る。
2. **差分が「移動と可視性の修飾子」だけである**: 分割前後でメソッドの本体を正規化して比較する（例: 移動前後の `check` 関連のソースから `pub(super)` を除き、メソッド単位で並べ替えて比較するスクリプトを `scripts/` に置く）。
3. S0 で保存した、全サンプルへの `sigil check`・`sigil spec --json` の出力と、バイト単位で一致する。
4. ネイティブ出力の回帰（`scripts/native-report.sh`）が現行の閾値（`SIGIL_NATIVE_MIN_CHECKED=400`）を満たす。

人間のレビューは不要（仕様に影響しない）。別系統のエージェントが、手順 2 の機械的な比較の結果を確認する。

## 6. 分割後の追加作業（優先順）

同じ方法を、衝突が多い順に適用する。**ファイルが異なるため、これらは並列に進められる**。

| 順 | ファイル | 行数 | 分割案 | レーン |
|---|---|---|---|---|
| 1 | `lower.rs` | 2,198 | `lower/{expr,stmt,pattern,closure,items}.rs` | C1 |
| 2 | `parser.rs` | 1,924 | `parser/{items,types,expr,stmt,pattern}.rs` | A1 |
| 3 | `codegen/func.rs` | 1,671 | `codegen/{func,expr,call,drop}.rs` | C2 |
| 4 | `ir_eval.rs` | 1,513 | `ir_eval/{expr,call,value}.rs` | C1 |
| 5 | `project.rs` | 1,227 | `project/{load,graph,resolve}.rs` | D1 |
| 6 | `fmt.rs` | 1,110 | `fmt/{items,expr,types}.rs` | A2 |

## 7. 完了の定義

- `src/check/` に 15 前後のファイルがあり、各ファイルが 600 行程度以下（`tests.rs` を除く）。
- 公開 API のパスが変わっていない（`crate::check::check` など）。
- すべての検証（§5）が通っている。
- [PARALLEL_DEV.md](PARALLEL_DEV.md) §3 のレーン表と、ファイルの割り当てが一致している。

