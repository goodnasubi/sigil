# コンテナ隔離: Docker Desktop の Enhanced Container Isolation

Docker Desktop の Enhanced Container Isolation（ECI）について、公式ドキュメントを基に目的・仕組み・制約と Sigil への示唆をまとめる。既存の仕様や ADR を変更する設計決定ではなく、実行環境を検討するための参考資料である。

## 目的と保護の境界

ECI は、悪意のあるコンテナが Docker Desktop の Linux VM やホスト OS を侵害したり、管理者が定めたレジストリ・プロキシなどの設定を改変したりするリスクを抑える機能である。Linux capabilities の削減、seccomp、AppArmor と組み合わせて隔離を強める。

ここでいう VM は Docker Desktop がコンテナを動かす Linux 環境であり、利用者のホスト OS と区別する。コンテナ内の root を対応付ける先は、**この VM 内の非特権ユーザー**である。ホスト OS 上のユーザーへ直接対応付けるという意味ではない。

## 主な仕組み

| 仕組み | 隔離への効果 |
|---|---|
| Sysbox ランタイム | ECI 有効後に `docker run` / `docker create` で作成するユーザーコンテナは、通常の runc の代わりに、runc を基に隔離を強化した Sysbox を自動的に使う。Docker の既定ランタイム自体は runc のままだが、`--runtime` 指定で ECI を回避することはできない。 |
| Linux ユーザー名前空間 | コンテナごとに重複しない UID 範囲を割り当て、コンテナ内 root を VM 内では非特権 UID に対応付ける。コンテナ内の権限が VM 全体の root 権限へ直結するのを防ぐ。 |
| 特権コンテナの封じ込め | `--privileged` 自体は禁止せず、その権限が及ぶ資源をコンテナの境界内に制限する。Docker-in-Docker などを支えつつ、カーネルモジュールの読み込みや VM 全体の BPF 設定変更などは拒否する。 |
| 名前空間共有の制限 | VM と PID・ネットワーク名前空間を共有する指定を拒否し、`--userns=host` によるユーザー名前空間の解除も許さない。 |
| マウントの制限 | 通常のホスト側ファイル共有は維持しつつ、VM の Docker 設定など機密性の高いパスを bind mount する操作を拒否する。Docker Engine を操作できる `/var/run/docker.sock` のマウントも既定で拒否する。 |
| システムコールの傍受 | 境界を越え得る制御系の呼び出しを Sysbox が検査する。たとえば `CAP_SYS_ADMIN` があっても、読み取り専用 bind mount を書き込み可能に変更する操作は拒否する。一方、コンテナ内部のファイルシステムへのマウントは可能である。 |
| ファイル所有者の対応付け | コンテナごとの UID 範囲の違いを、ID-mapped mounts などによるファイルシステム側の対応付けで吸収する。異なる UID 範囲のコンテナ間でもボリュームを共有できるようにする。 |
| `/proc`・`/sys` の仮想化 | 一部の情報をコンテナごとの表示に置き換える。たとえば稼働時間は VM 全体ではなくコンテナの値を見せる。カーネルが名前空間で分離していない一部の `/proc/sys` もエミュレートし、VM の情報露出と設定への影響を抑える。 |

システムコール検査は主に制御系を対象とし、通常のデータ処理への影響を抑える設計である。ただし、公式 FAQ は `mount` / `umount` が多い処理では影響があり得るとしており、すべての負荷で性能差がないとまでは捉えない。

## 前提と制約

- **契約と有効化**: Docker Business 契約が必要。組織として設定を強制する管理者運用ではサインインの強制も前提となる。Settings Management で有効化と設定のロックを行える。
- **既存コンテナ**: 有効化前に作成したコンテナは保護されないため、作り直す必要がある。
- **Linux コンテナ限定**: ネイティブ Windows コンテナのモードには対応しない。
- **WSL 2**: WSL 2 バックエンドで ECI を使うには WSL 2.6 以上が必要。ECI は Linux カーネル 6.3.0 以上を必要とし、WSL 2.6 は 6.6 を含む。Docker Desktop 自体の要件である WSL 2.1.5 以上と混同しない。
- **WSL と Hyper-V の違い**: WSL では利用者が `docker-desktop` ディストリビューションへ直接入り、VM 内の root として設定を変更できる。また WSL ディストリビューション間でカーネルを共有する。コンテナの隔離は強化されるが、利用者による VM への直接アクセスや他ディストリビューションからの影響まで防げるわけではない。公式資料は隔離を優先する場合に Hyper-V を推奨している。
- **保護対象外**: Docker Extensions のコンテナ、Docker Debug のコンテナ、組み込み Kubernetes の旧 `kubeadm` 方式は保護対象外。`kind` 方式では、ECI 有効時に各ノードが保護されたコンテナとして動く。
- **ビルド**: `docker-container` ドライバーは保護対象。既定の `docker` ドライバーは Docker Desktop 4.30 以降で保護されるが、WSL 2 は例外となる。ホストネットワークや `network.host`・`security.insecure` entitlement を使うビルドにも制約があるため、ドライバーとバージョンを含めて確認する。
- **Docker ソケットの例外**: 管理者は信頼するイメージにマウントを許可し、使用可能なコマンドを制限できる。ただし、コマンド制限は許可した全イメージに共通で、イメージごとには分けられない。許可範囲の拡大は保護境界を変えるため、必要な範囲に絞る。
- **互換性**: カーネル全体の設定変更、直接のデバイスアクセスなどを前提とする処理は動作が変わり得る。開発時の Sysbox と、本番で使うランタイムの差も確認が必要である。

保護範囲や例外設定は Docker Desktop のバージョンで変わる。導入時には末尾の公式資料で再確認する。

## Sigil との関係

1. **G3 レビューの「イメージ」を支える考え方**: [ADR 0020 決定 3 の項目 10](../decisions/0020-p1-security-addendum.md) は、root でない数値 ID での実行と、シェルなどを含まず実行ファイルを 1 つに絞る構成を求める。ECI は、実行主体の権限が外へ広がるのを防ぐ業界側の実装例として参考になる。ただし ECI はコンテナ内 root も扱う機能であり、非 root 実行や最小イメージの代わりにはならず、イメージ内の実行ファイル数も保証しない。イメージの権限・攻撃面を減らす対策と、実行基盤側で境界を強制する対策を重ねる。
2. **言語の外側の実行環境**: [ADR 0027 の追記](../decisions/0027-ai-era-security-design-principles.md) が挙げる「実行環境」の層を考える具体例になる。Sigil 内での保証に加えて、ランタイム・OS 側で権限、マウント、名前空間を制限することで、将来サービスを外部公開する際の防御を重ねられる。ただし Docker Desktop の ECI 設定が、そのまま本番の隔離を保証するものではない。本番環境でも保護範囲を別途確認する必要がある。
3. **将来のアプリケーション開発の参考**: Docker Desktop のようにコンテナ実行を管理するアプリを将来作る場合、利用者の通常操作を保ちながら管理者の方針を強制する設計が参考になる。ランタイムの選択、例外の限定、保護対象外の明示を、管理機能と一緒に考える材料となる。

## 出典

以下の Docker 公式ドキュメントを、自分の言葉で要約した。文書サイトへのアクセスが制限されたため、内容は Docker 公式 `docker/docs` リポジトリの対応する Markdown で確認した。

- [Enhanced Container Isolation（概要と仕組み）](https://docs.docker.com/desktop/enterprise/hardened-desktop/enhanced-container-isolation/)
- [Enable Enhanced Container Isolation（前提と有効化）](https://docs.docker.com/desktop/enterprise/hardened-desktop/enhanced-container-isolation/enable-eci/)
- [Enhanced Container Isolation limitations（制約と保護対象）](https://docs.docker.com/desktop/enterprise/hardened-desktop/enhanced-container-isolation/limitations/)
- [Enhanced Container Isolation FAQs（互換性・性能・保護対象）](https://docs.docker.com/desktop/enterprise/hardened-desktop/enhanced-container-isolation/faq/)
- [Configure Docker socket exceptions and advanced settings（例外設定）](https://docs.docker.com/desktop/enterprise/hardened-desktop/enhanced-container-isolation/config/)
- [確認した公式ソースの版](https://github.com/docker/docs/tree/f081e781d06dd45e9f8ce1a19142c29205bc9777/content/manuals/desktop/enterprise/hardened-desktop/enhanced-container-isolation)
