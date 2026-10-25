<div align="center">
  <!-- 💚 お手持ちのロゴ画像をリポジトリにアップロード後、srcのパスを書き換えてください -->
  <img src="sigil-logo-mint.jpg" alt="Sigil Logo" width="220">

  <h1>💚 Sigil Programming Language ✨</h1>

  <p><i>"Powered by Mint, Sweetening Code."</i></p>

  <img src="https://shields.io" alt="Style"/>
  <img src="https://shields.io" alt="License"/>
  <img src="https://shields.io" alt="Version"/>
</div>

---

### 🍵 ようこそ、Sigilの世界へ！
**Sigil**は、あの愛らしい緑のちいさな相棒がフォースで引き寄せた**お気に入りのミントマカロン**のように、ひとくちで心をときめかせる**心地よさと美しさ**を大切にした新しいプログラミング言語です。

コードを書く時間が、もっと楽しく、もっとあなたらしくなるように。魔法の印（Sigil）を紡ぐように、お気に入りのロジックをデザインしてみませんか？

---

## 🎀 3つのときめきポイント (Features)

* **ミント・セーフティ (Refreshing Safety)**
  爽快でタフな静的型チェック。バグを事前に優しくキャッチし、あなたの開発ライフの安心をフォースのようにそっと守ります。

* **マカロン・シンタックス (Sweet Syntax)**
  甘くてシンプル。無駄な記号をそぎ落とし、まるで可愛いブログのダイアリーを読んでいるかのように滑らかに頭に入ってくるコードデザインです。

* **パステル・エコシステム (Pure & Light Ecosystem)**
  余計なものは一切なし。ピュアで軽量なコアランタイムにより、どんな環境でもサクサクと軽快に動き出します。

---

## 🎨 コードのぞき見 (Hello, Sigil!)

まずは一番シンプルな魔法を使ってみましょう。爽やかなミントミルクのように優しいコードの佇まいを感じてください。

```sigil
// 💚 はじめての魔法
cast spell invocation() {
    let greeting = "Hello, Sweet Mint World!"
    print(greeting)
}

invocation()
```

---

## 🚀 はじめかた (Quick Start)

あなたのお手元の環境に、またたく間にSigilをインストールできます。

### 1. 魔法陣の準備 (Installation)
ターミナルを開いて、以下のコマンドをそっと実行してください。

```bash
curl -fsSL https://sigil-lang.org | sh
```

### 2. スクリプトの実行 (Run Your First Spell)
拡張子 `.sigil` のファイルを作ったら、いよいよ起動です。

```bash
sigil run welcome.sigil
```

---

## 🍰 カラーパレットと世界観 (Design Identity)

Sigilのコミュニティやツールチェーンは、以下の**ミント＆マカロンパステル**な世界観で統一されています。

| カラー名 | カラーコード | 役割 |
| :--- | :--- | :--- |
| **Nevarro Mint** | `#BFE3DB` | メインアクセント・キーワード（グローグーのマカロン色） |
| **Macaron Pink** | `#FFD1DC` | サブアクセント・型名 |
| **Lemon Yellow** | `#FFFFBA` | ストリング・リテラル |
| **Lavender Spell** | `#E8DCFA` | 関数名・ビルトイン魔法 |

---

## 🤝 一緒にお茶しませんか？ (Contributing)

Sigilはみんなで育てるオープンソースの秘密基地です。
「このシンタックスをもっと可愛くしたい！」「このエラーメッセージを優しくしたい！」といったアイデアがあれば、いつでもお気軽に Pull Request や Issue を投げてくださいね。

1. このリポジトリを **Fork** する 🍴
2. あなたのアイデアを **Branch** にまとめる 🌿
3. 楽しんでコードを書き、**Pull Request** を送る 🎉

---

<div align="center">
  <p>Made with 💚 and a cup of mint tea.</p>
  <p>Copyright © 2026 Sigil Core Team. Licensed under the MIT License.</p>
</div>


---

# Sigil

**人間に代わって AI がコーディングするための開発言語。**

Sigil（シジル）は、Java・React・Node.js に代わる言語を目指して開発する、AI 専用の汎用プログラミング言語です。
コードは AI が書き、AI が保守します。人間は **仕様だけ** をレビューします。

> **ステータス:** 設計段階が中心で、処理系（Rust 製）は M0〜M1 まで実装済みです。M0 は構文解析・`sigil fmt`・`sigil spec`、M1 は型検査と効果の検査（標準ライブラリは仮の最小範囲 `src/stdlib.rs`）で、5 つの例が `sigil check` を通ります。M2（所有権・借用の検査）は進行中で、参照を書ける位置の規則と、`&T` の受け手のメソッドに改めた標準ライブラリまで入っています、値の移動の検査（分岐・ループ・部分的な移動・match による移動、移動した後の使用）まで入っています（借用の解析は未着手）。M3 以降は未着手です。
> P0（言語の核）の実装の進め方（Rust で書くコンパイラ、段階 M0〜M6）を [ADR 0006](docs/decisions/0006-p0-scope-and-bootstrap.md) で決めました。
> 並行処理のモデル（中断を効果として扱う構造化並行処理）を [ADR 0007](docs/decisions/0007-concurrency-model.md) で決めました。
> M0 の前に決める表記（`old`・篩型・モジュールの宣言・`sigil spec` の出力など）を [ADR 0008](docs/decisions/0008-m0-notation.md) で決めました。
> 字句と文法の全体仕様（コメント・識別子・予約語・リテラル・演算子の優先順位・正規形）を [ADR 0009](docs/decisions/0009-lexical-and-grammar.md) で決めました。
> `sigil fmt` の仕様と M0 の補足を [ADR 0010](docs/decisions/0010-fmt-and-m0-addendum.md) で、型システムの範囲（所有権と借用の表記、型引数、篩型、関数の値の制限など）を [ADR 0011](docs/decisions/0011-type-system-scope.md) で決めました。

## 目標

- **AI が書き手**: 人間の書きやすさではなく、AI が意図を曖昧さなく表現でき、誤りを機械的に検出できることを優先します。
- **人間は仕様だけを見る**: 意図・事前条件・事後条件・効果をコードから取り出して人間に示し、コードがそれを満たすことを処理系が検証します。
- **フロントエンドからバックエンドまで 1 言語**: ブラウザの UI（React の置き換え）、サーバー（Java・Node.js の置き換え）、その間の通信を同じ言語で書きます。
- **独自の実行方式**: コンパイラと実行環境を独自に作ります。サーバー向けにはネイティブコード、ブラウザ向けには WebAssembly と JavaScript を出力する想定です。
- **Python から呼び出せる**: Sigil で書いた関数・型を Python から呼び出せるようにします。
- **Rust と同等のメモリ安全性**: 解放後の使用・ヌル参照・データ競合などのメモリ不具合を、コンパイル時に排除します。

決定の経緯は [docs/decisions/0001-vision-and-scope.md](docs/decisions/0001-vision-and-scope.md) を参照してください。

## 長期目標

最終的には、Sigil で次のものを置き換えられるようにすることを目指します（[ADR 0005](docs/decisions/0005-long-term-goals.md)）。

- Next.js（フロントエンドとサーバーを一体で書く Web フレームワーク）
- Spring（Java のサーバーアプリケーション用フレームワーク）
- Python
- データベース
- Git（ソースを管理するリポジトリ管理）
- サービスを Docker コンテナで動く形にしてリリースする仕組み、さらにコンテナを管理する仕組み
- OS（VMware ESXi 上の仮想マシンで動く OS を含む）
- IDE（統合開発環境）: 人間には仕様をレビューする場、AI にはコードを書くための道具を提供する
- Web ブラウザ

進める順序:

| 段階 | 内容 |
|---|---|
| P0 | 言語の核（コンパイラ・検査・仕様の表示・標準ライブラリの最小部分・言語サーバー） |
| P1 | サーバー（Spring の置き換え）と Docker コンテナでのリリース |
| P2 | Web フルスタック（Java・React・Node.js・Next.js の置き換え）と IDE |
| P3 | Python の置き換えと Python 連携 |
| P4 | データベース・リポジトリ管理（Sigil で書くアプリケーション） |
| P5 | コンテナ管理 |
| P6 | OS（ESXi 上の仮想マシンを含む） |
| P7 | ブラウザ（P2 の後、P3〜P6 と並行して進められる） |

## 設計の柱（草案）

1. **仕様の明示**: 関数や単位ごとに、意図・前提・事後条件・効果を一級の構文として書く。
2. **機械的検証**: 型・所有権・契約・効果の検査で、誤りを実行前に検出する。
3. **決定的な表現**: 同じ意味を複数の書き方で表せないようにする（正規化可能な構文）。
4. **人間の可読性より検証可能性**: 人間が読むのは仕様であり、コード本体の読みやすさは優先しない。

詳細は [docs/DESIGN.md](docs/DESIGN.md) を参照してください。

## リポジトリ構成

```
.
├── README.md          このファイル
├── LICENSE            MIT License
├── docs/
│   ├── DESIGN.md      言語設計の全体像（草案）
│   └── decisions/     設計判断の記録（ADR）
└── examples/          サンプルコード（構文の検討用）
```

## ライセンス

MIT License。詳細は [LICENSE](LICENSE) を参照してください。
