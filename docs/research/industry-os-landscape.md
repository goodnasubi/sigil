# 業界横断 OS 調査レポート: 各業界の OS と、Sigil がターゲットにするには

- 調査日: 2026-11-16
- 目的: Sigil（AI エージェント向けの安全性重視の言語）の長期方針の材料として、自動運転以外の業界について「どんな OS が使われ、どんな安全規格の下にあるか」「Sigil がその業界をターゲットにするには何が要るか」を整理する。自動運転 OS 調査レポート（同日作成）の横展開であり、特に長期段階 P6（ハードウェアの上で直接動く OS 本体、フィジカル AI 向け組み込み OS を含む。ADR 0005）の設計論点につなげる。
- 調査方法: Web 検索と、ベンダー公式資料・標準化団体寄りの公開情報の確認を中心とした。認証に関する記述は、第三者認証機関（TÜV 各社、SGS、exida 等）への言及がある情報に基づく。ベンダーの自己申告（「certifiable」「対応可能」等）と、実際に発行された認証は区別して記す。
- 前提: 自動運転の回の結論（安全側の土台は認証済み RTOS / separation kernel、Sigil の現実的な最初の適用点は AI 出力を検証し権限を絞る safety monitor 的 component）は本レポートでも検証対象とし、他業界でも同じ構図が成り立つかを業界ごとに見た。

---

## 1. 概要

業界を横断して見ると、構図は自動運転とほぼ同じだった。違いは「どの規格が支配的か」「認証の重さ」「OS に何が求められるか」の配分である。

主要な発見は次のとおり。

- **安全側の土台の顔ぶれは業界をまたいでほぼ共通**である。QNX、INTEGRITY（Green Hills）、PikeOS（SYSGO）、VxWorks（Wind River）の 4 つが、自動車・航空・医療・産業・鉄道・エネルギーを横断して現れる。これに Eclipse ThreadX（旧 Azure RTOS）が、IEC 61508 SIL 4 / ISO 26262 ASIL D / EN 50128 SIL 4 / IEC 62304 Class C という横断認証を持って加わる。「業界ごとの OS」ではなく「**少数の認証済み基盤が、業界ごとの規格ラベルを取り直して横断する**」のが実態である。
- **規格の側も同型**である。IEC 61508（SIL）を傘として、自動車は ISO 26262（ASIL）、鉄道は EN 50128（SIL）、医療は IEC 62304（Class）、航空は DO-178C（DAL）と、業界規格が派生する。原子力だけは IEC 61513 / IEC 60880 という独自の体系を持ち、保護系では software 自体を避けて FPGA や relay logic に寄せる文化が残る点で異質である。
- **Linux の位置は業界で共通**している。高性能・非安全側では事実上の標準だが、安全側では「そのまま」ではなく、(a) hypervisor 上の guest として隔離する、(b) 外部の高信頼 monitor で監視する、(c) SEooC（Safety Element out of Context）として定義済み構成だけを認証する、のいずれかの形でしか入らない。Red Hat In-Vehicle Operating System の ISO 26262 ASIL B 認証（exida、SEooC 方式、2025 年 GA）が、その到達点の目安である。航空の IMA では Linux は DAL A/B の土台としては不適とする議論が公然と行われている。
- **オープンソース基盤にも認証の道はある**が、形が決まっている。RTEMS は Qualification Data Package（QDP）で ECSS 準拠の証跡を有償提供する形、Eclipse ThreadX は code は MIT で公開し認証 artifact を有償会員制にする形、FreeRTOS は認証版 SafeRTOS を別製品として切り出す形である。「code が開いていること」と「認証の証跡が使えること」は別商品である。
- **認証の重さには明確な序列**がある。原子力 Category A ≒ 航空 DAL A ≒ 鉄道 SIL 4 が最重級で、言語・tool の新規参入が最も難しい。逆に小型ドローン、船舶（型式承認型）、IoT は software の lifecycle 規格による縛りが弱く、新しい言語が実績を積む余地が大きい。Sigil の参入順序はこの序列を逆向きにたどるのが現実的である（第 5 章）。

---

## 2. 業界別の一覧表

| 業界 | 代表的な OS / 基盤 | 支配的な安全規格・認証 | OS に求められる性質 |
|---|---|---|---|
| 航空（民間） | VxWorks 653、INTEGRITY-178、LynxOS-178、PikeOS、DeOS | DO-178C（DAL A–E）、ARINC 653（partitioning）、DO-254（hardware） | 時間・空間の静的 partitioning、長期（数十年）の構成固定、形式手法の受容 |
| 宇宙 | RTEMS、VxWorks、Linux（payload 側）、NASA cFS / F Prime は framework 層 | ECSS-E-ST-40C / ECSS-Q-ST-80C（criticality category A–D）、NASA の class 区分 | radiation 環境での信頼性、FDIR、打ち上げ後の不具合修正の困難さへの対応、長寿命 |
| 防衛 | 航空と同系（VxWorks、INTEGRITY、LynxOS）に加え MILS 系 separation kernel | DO-178C + FACE / MOSA（portability の標準化）、Common Criteria（EAL） | multi-level security、相互運用性と部品の再利用、供給の継続性 |
| 医療機器 | QNX、VxWorks、INTEGRITY、Linux（画像・監視系）、FreeRTOS / ThreadX（小型機器） | IEC 62304（Class A / B / C）、FDA 510(k) / PMA、ISO 14971 | lifecycle process の証跡、SOUP（既製 software）の管理、7–15 年の長期 support、cybersecurity（SBOM） |
| 産業制御・FA・プロセス | PLC runtime（CODESYS 等）が OS の上に載る構造。VxWorks、Linux、Windows 系、専用 firmware | IEC 61508（SIL 1–4）、IEC 61131-3（PLC 言語）、IEC 61511（プロセス SIS） | scan cycle の決定性、安全 controller の分離、数十年単位の保守と互換性 |
| ロボット | ROS / ROS 2（middleware、OS ではない）+ Linux、安全側は safety PLC / 認証済み RTOS、QNX（NVIDIA Halos for Robotics） | ISO 10218（産業用、2025 年改訂）、ISO/TS 15066（協働）、ISO 13482（personal care）、ISO 3691-4（AGV / AMR）、ISO 13849（PL） | AI 側と安全側の二重化、人との近接を前提にした停止・制限機能、規格自体が形成期 |
| 鉄道 | PikeOS、VxWorks 系、Ada / SCADE 等の言語基盤と専用 platform | EN 50126 / 50128 / 50129（SIL、信号は SIL 4）、EN 50657（車上） | 30 年超の運用、fail-safe 原則、形式手法・モデルベース開発の文化 |
| エネルギー（電力） | QNX（変電所 IED、turbine 制御）、VxWorks、Linux（SCADA 上位系） | IEC 61850（通信）、NERC CIP（security）、系統連系の各国規格 | 冗長化と failover、長期連続運転、security 規制への対応 |
| エネルギー（原子力） | 専用の小規模 system software、FPGA / relay logic（保護系） | IEC 61513（system）、IEC 61226（Category A / B / C）、IEC 60880（Category A software）、IEC 62138（B / C） | 共通原因故障への多様性、検証可能性の極端な重視、software を避ける設計文化 |
| 建設機械・農業機械 | 専用 ECU + 小型 RTOS（PXROS-HR 等）、CODESYS、高性能側に QNX + ROS 2 | ISO 13849（PL）、ISO 25119（AgPL、農業）、ISO 19014（MPL、土工機械） | 過酷環境、mixed criticality を 1 つの ECU で、SIL 2 / PL d 級が中心 |
| 船舶・海洋 | 商用 OS（Windows IoT 系、Linux）が航海機器に広く使われる。機関制御は専用 system | SOLAS / IMO 性能基準、IEC 61174（ECDIS）等の型式承認、船級協会（DNV 等）の承認 | 機器単位の型式承認が中心で、software の integrity level による階層化は弱い。冗長性と可用性 |
| ドローン / UAV | PX4 + NuttX、ArduPilot + ChibiOS（小型）、大型・認証型は航空系 stack へ収束 | 小型はほぼ無認証（電波・飛行規制中心）。大型 / eVTOL は DO-178C 等の航空規格へ | 小型は開発速度と生態系、大型は航空と同じ認証。落差が最も大きい業界 |
| 通信インフラ・ネットワーク機器 | Linux（SONiC 等の network OS を含む）、RTOS（基地局の L1 処理等） | 安全規格ではなく可用性・security・相互接続性が中心。通信事業者の独自要件 | 長期連続運転、in-service upgrade、処理性能 |
| 家電・IoT | FreeRTOS、Zephyr、Eclipse ThreadX、NuttX、Linux（高機能側） | IEC 60335 / UL 1998（家電 software の Class B / C）、製品安全と security（ETSI EN 303 645 等） | 量産コスト、OTA、security。安全は「単一故障で危険にならない」水準が中心 |

---

## 3. 業界別の詳細

### 3.1 航空（民間）

航空は安全 software の作法が最も早く固まった業界であり、他業界の規格の手本になっている。支配的なのは **DO-178C**（software の lifecycle、DAL A が最重）と、統合型 avionics のための **ARINC 653**（時間・空間 partitioning と APEX API の標準）である。

代表的な OS は ARINC 653 準拠の partitioning kernel で、**VxWorks 653**（Wind River。多 core での DAL A 認証の実績）、**INTEGRITY-178**（Green Hills。security 側で Common Criteria EAL 6+ の評価実績を持ち、FACE 3.0 への適合認証でも先行）、**LynxOS-178**（Lynx。POSIX 互換性の高さが売り）、**PikeOS**（SYSGO。航空と鉄道の両方で認証実績）、**DeOS**（DDC-I）である。共通する性質は、静的な時間配分（cyclic scheduling 的な時間窓の固定）、partition 間の干渉の排除、そして数十年単位で構成を変えない運用である。

近年の軸は **FACE Technical Standard** と **MOSA**（Modular Open Systems Approach）による部品の portability 標準化で、防衛と共通する（3.3 参照）。Linux については、IMA の DAL A/B の土台としては「決定性と証跡の面で不適」とする技術論文が出るなど、安全側への参入は進んでいない。

### 3.2 宇宙

宇宙の software は **ECSS**（欧州）では ECSS-E-ST-40C（software engineering）と ECSS-Q-ST-80C（product assurance）が支配的で、criticality category A–D で要求が変わる。米国では NASA の framework である **cFS**（core Flight System、OSAL による OS 抽象化）と JPL の **F Prime** が framework 層を占める。OS 層の代表は **RTEMS**（オープンソース。商用 support 会社が ECSS 向け Qualification Data Package を提供し、category C（tailored B）までの資格付けを支える）、**VxWorks**（探査機系での採用実績）、そして payload 側での embedded Linux である。

宇宙固有の事情は、(a) 打ち上げ後に物理的に触れないため FDIR（故障検出・分離・復旧）の自律性が要る、(b) 放射線による bit 反転を前提にする、(c) 開発期間と運用期間がともに長い、の 3 点である。近年は小型衛星の payload software で「信頼できない code を分離して載せる」需要が生まれており、seL4 のような検証済み separation kernel や hypervisor による hosted payload の分離が議論されている。認証の重さ自体は航空より緩い場面も多く（mission の class による）、新しい基盤の実験場になりやすい。

### 3.3 防衛

防衛の航空系は民間航空とほぼ同じ stack（DO-178C、ARINC 653、同じ RTOS 群）を使い、そこへ 2 つの層が加わる。第一に **security** の層で、MILS（Multiple Independent Levels of Security）の考え方と Common Criteria の高い EAL が separation kernel に求められる（INTEGRITY-178B の EAL 6+ が代表例）。第二に **portability と調達** の層で、米国では MOSA が主要防衛調達 program への適用を法的に求められ（2019 年の Tri-Service memo、Title 10）、FACE が software 部品の標準環境を定める。地上車両では英国の GVA（Generic Vehicle Architecture）のような vehicle architecture 標準がある。

Sigil の観点では、防衛は「認証 + security 評価 + 長期供給」の複合で参入障壁が高く、部品の portability 標準（FACE の OSS profile のような OS API 標準）への適合が入口になる業界である。

### 3.4 医療機器

医療の中心規格 **IEC 62304** は、OS や言語そのものを認証する規格ではなく、software の **lifecycle process** を Class A / B / C（危害の大きさ）に応じて要求する規格である。ここが自動車・航空と違う。既製品の software は **SOUP**（Software of Unknown Provenance）として扱われ、Linux を使う場合は製造者側が SOUP の評価・監視の負担を引き受ける。逆に **QNX OS for Safety** のように IEC 62304 Class C への事前認証を持つ基盤を使うと、その分の文書負担が減る、という売り方が成立している。

OS の実態は、画像診断・患者 monitor のような高機能機器では Linux と商用 RTOS の併用、手術 robot では QNX や INTEGRITY のような分離の強い基盤、小型・携帯機器では FreeRTOS や ThreadX が中心である。機器の寿命は 7–15 年と長く、FDA の cybersecurity guidance（2026 年版では SBOM と更新計画の提示が強く求められる）により、security と更新可能性が OS 選定の表の項目になっている。認証の「重さ」は process の重さであり、言語・tool については tool の妥当性確認と SOUP 管理が論点になる。

### 3.5 産業制御・FA・プロセス産業

産業の傘は **IEC 61508**（SIL 1–4）で、多くの業界規格の親規格である。この業界の特徴は、「OS の上に PLC runtime が載り、その上で IEC 61131-3 の言語（ladder、function block 等）が動く」という層構造が標準化されていることである。**CODESYS** はその代表で、runtime は Linux、VxWorks、OS なし（bare metal）など複数の土台に移植され、安全側には IEC 61508 SIL 3 の事前認証を受けた safety 拡張や、Linux container 内で動く virtual safety controller（diversified encoding による二重化、TÜV SÜD 認証）まである。プロセス産業の安全計装（SIS）は IEC 61511 の下で Triconex、HIMA、Yokogawa ProSafe-RS 等の専用 safety system が SIL 3 を担い、汎用 OS の出番は限られる。

OS に求められるのは scan cycle の決定性と、数十年単位の保守・互換性である。言語の観点では、IEC 61131-3 という「業界専用言語 + 認証済み runtime」の前例があることが重要で、新しい言語が runtime ごと入り込む余地が他業界より構造的に大きい。

### 3.6 ロボット

ロボットの software の事実上の標準は **ROS / ROS 2** だが、これは middleware であって OS ではなく、安全認証も持たない。安全規格は **ISO 10218**（産業用 robot、2025 年改訂で安全機能は SIL 3 までを扱う）、**ISO/TS 15066**（協働 robot）、**ISO 13482**（personal care robot）、**ISO 3691-4**（AGV / AMR）、そして制御系の **ISO 13849**（PL）が重なる。

実態の構成は自動車と同型で、「ROS 2 + Linux の AI・計画側」と「safety PLC や認証済み RTOS の安全側（非常停止、速度・力の制限）」の二重化である。NVIDIA が **Halos for Robotics** として、hypervisor で Linux VM（AI）と QNX VM（安全機能）を分ける構成を打ち出したことは、この構図が physical AI の標準形になりつつある証拠である。一方で、動的歩行する humanoid 向けの安全規格（ISO 25785）はまだ策定中で、**規格自体が形成期**である点が他業界と決定的に違う。既存の認証済み部品が埋めていない隙間が大きく、新しい言語・基盤が入り込む余地が最も大きい業界の一つである。

### 3.7 鉄道

鉄道は **EN 50126 / 50128 / 50129**（CENELEC 系、software は EN 50128、車上は EN 50657）で、信号 system は **SIL 4** が求められる最重級の業界である。運用期間は 30 年を超え、fail-safe（故障時は安全側に倒れる）原則が設計の中心にある。OS では **PikeOS** が EN 50128 / EN 50657 SIL 4 の事前認証を公式に持ち、連動装置や列車制御での採用例が多い。言語の文化も特徴的で、SCADE のようなモデルベース開発や Ada / SPARK のような検証しやすい言語が好まれてきた。「言語の選択が認証戦略の一部」という点で、Sigil にとって示唆の多い業界だが、参入の重さも最大級である。

### 3.8 エネルギー

**電力**では、変電所 automation の通信規格 IEC 61850 と、IED（Intelligent Electronic Device）や turbine 制御の基盤として QNX や VxWorks が使われる（GE の変電所 controller や Mark VIe turbine 制御は QNX 系）。安全規格というより、冗長化・failover と長期連続運転、そして NERC CIP のような security 規制が OS の要件を決める。風力発電の controller も同様に商用 RTOS と専用 controller が中心である。

**原子力**は別格である。IEC 61513（system 全体）、IEC 61226（機能の Category A / B / C 分類）、IEC 60880（Category A の software。2026 年に IEC/IEEE 60880 として改訂）、IEC 62138（Category B / C）という体系で、最も重要な保護系では software の共通原因故障を嫌い、relay logic や FPGA に寄せる設計が今も選ばれる。汎用 OS の V&V（検証・妥当性確認）の困難さ自体が文献で論点になっており、「小さく、単純で、検証し尽くせる system software」以外は土台になりにくい。Sigil の設計原則と方向は合うが、実績要求の高さから当面の適用点は監視・診断等の周辺に限られる。

### 3.9 建設機械・農業機械（off-highway）

建機・農機は自動車と産業の中間にあり、規格は **ISO 13849**（PL、機械全般）、**ISO 25119**（AgPL、農業機械）、**ISO 19014**（MPL、土工機械）が重なる。求められる水準は SIL 2 / PL d 級が中心で、自動車の ASIL D より一段軽い。実態は専用 ECU 上の小型 RTOS で、TTControl の ECU 群と PXROS-HR（TÜV Nord 認証、SIL 2 / PL d / AgPL d / ASIL C まで）のような「ECU + 認証済み RTOS + CODESYS」の package が典型である。高性能側では Bosch Rexroth の ORC2 のように QNX + ROS 2 で自動化・自律化に備える動きも出ている。認証の中程度の重さと、自動化需要の伸びの組み合わせは、Sigil の適用点として現実味がある。

### 3.10 船舶・海洋

船舶は他業界と制度が違う。SOLAS 条約と IMO の性能基準の下で、ECDIS（IEC 61174）のような**機器ごとの型式承認**を船級協会（DNV 等）が行う方式が中心で、software を integrity level で階層化する文化は弱い。航海機器の OS は商用 OS が広く使われ（DNV の型式承認証書の例では ECDIS の OS として Embedded Windows / Windows 10 IoT Enterprise LTSC が記載される）、機関・推進の制御は Kongsberg や Wärtsilä 等の専用 system である。自律運航（MASS）は IMO で規則の検討が進む段階で、安全 software の作法はまだ固まっていない。新しい言語にとっては「認証の壁は低いが、市場が機器 package 単位で閉じている」という業界である。

### 3.11 ドローン / UAV

ドローンは業界内の落差が最大である。小型機では **PX4**（Apache NuttX 上の autopilot、Dronecode Foundation）と **ArduPilot**（ChibiOS 上。Linux 上でも動作）がオープンソースの二大 stack で、MAVLink が通信の標準である。この層に安全認証はほぼ存在せず、規制は飛行ルールと電波が中心である。一方、大型 UAS や eVTOL（Joby、Archer、Volocopter、Wisk 等）は有人航空と同じ世界に入り、VxWorks 653、INTEGRITY-178、LynxOS-178 と DO-178C の stack に収束する。つまりドローンは「無認証のオープン生態系」と「最重級の航空認証」が同居し、前者で実績を積んで後者へ上がる梯子が業界内にある。

### 3.12 通信インフラ・ネットワーク機器、家電・IoT

通信・ネットワーク機器は安全規格の世界ではなく、可用性と security と性能の世界である。switch の network OS は Linux 系（SONiC 等のオープン化も進む）、基地局の実時間処理には RTOS が使われる。参入障壁は認証ではなく生態系と性能である。

家電・IoT の基盤は FreeRTOS、Zephyr、Eclipse ThreadX、NuttX と Linux の棲み分けで、安全は IEC 60335 / UL 1998 の software Class B / C（単一故障で危険な状態にならないことの確認）が中心になる。Zephyr は IEC 61508 SIL 3 の認証取得を進めており、オープンソース RTOS の認証化の流れがここでも進んでいる。量産数と更新（OTA）のしやすさが支配的で、言語にとっては「小さな footprint と長期保守」が問われる市場である。

---

## 4. 横断的な構図

### 4.1 認証済み基盤は少数で、業界を横断する

第 2 章の一覧を縦に読むと、同じ名前が繰り返し現れる。

| 基盤 | 航空 | 自動車 | 医療 | 産業 | 鉄道 | エネルギー・その他 |
|---|---|---|---|---|---|---|
| QNX | △ | ◎ ASIL D | ◎ IEC 62304 Class C 事前認証 | ◎ IEC 61508 SIL 3 | ○ | ◎ 電力 IED・turbine 制御 |
| INTEGRITY | ◎ DAL A（-178 系） | ◎ ASIL D | ◎ 手術 robot 等 | ◎ SIL 3 | ○ | 防衛（EAL 6+） |
| PikeOS | ◎ DAL A 対応 | ◎ ASIL D 対応 kit | ○ | ◎ SIL 4 対応 | ◎ EN 50128 SIL 4 事前認証 | 防衛・宇宙 |
| VxWorks | ◎ DAL A（653） | ○ ASIL D evidence | ◎ | ◎ | ○ | ◎ 宇宙・電力・風力 |
| Eclipse ThreadX | △ | ◎ ASIL D | ◎ Class C | ◎ SIL 4 | ◎ EN 50128 | 家電（UL 1998 / IEC 60730） |
| Linux | ×（IMA の安全側） | ◎ 非安全側 + ASIL B の SEooC 例 | ◎ SOUP として広く使用 | ◎ 非安全側 + 監視付き安全利用 | △ | ◎ SCADA・network・IoT |

（◎ = 中核的な採用・認証実績、○ = 採用・対応の実績、△ = 周辺的、× = 安全側の土台としては不適とされる）

つまり「業界ごとの OS 戦略」は幻想で、実態は「**少数の基盤ベンダーが、同じ kernel に業界ごとの認証ラベルと証跡 package を付けて売る**」である。参入する側（Sigil を含む）にとっての意味は二つある。第一に、競合は業界ごとではなく基盤の層で決まる。第二に、基盤そのものと競合せず、その上で動く component の層に入る道は全業界に共通して開かれている。

### 4.2 Linux の安全利用は「隔離・監視・限定」の 3 形

Linux の安全用途への広がり方は、業界を問わず次の 3 形に収束している。

1. **隔離型**: hypervisor / separation kernel の guest として Linux を閉じ込め、安全機能は外の partition に置く（自動車の Halos 構成、robot の Halos for Robotics）。
2. **監視型**: Linux の外に高信頼の monitor を置き、Linux の異常を検出して安全状態へ移す（EB corbos Linux の外部監視、産業の virtual safety controller の二重化 encoding）。
3. **限定・認証型（SEooC）**: 構成と hardware を固定した Linux distribution を Safety Element out of Context として認証する（Red Hat In-Vehicle Operating System の ASIL B。Linux Foundation の ELISA project がこの道の process と tool を整備している）。この形では、認証済みの Linux kernel 自体が memory・実行時間等の分離を担い、安全機能はその kernel 上で動く。

ただし 3 の認証型は、1・2 と構造が異なる。隔離型と監視型では安全を Linux の外側の小さな層が担うのに対し、認証型では認証済みの Linux 自体が分離を担い、安全機能はその上で動く。SEooC は「安全責務を外部の monitor へ移す」構成を意味しない。したがって Sigil の入るべき場所は「Linux の外側」だけではない。外側の小さな層に加えて、認証済み Linux の上で動く安全アプリケーションも適用先になり得る。いずれの場合も共通するのは、入るべき層が小さく検証可能でなければならないという点である。

### 4.3 認証は process と証跡の商品である

オープンソース基盤の事例（RTEMS の QDP、Eclipse ThreadX の認証 artifact 有償化、SafeRTOS の別製品化）が示すのは、認証の世界では **code そのものより、開発 process の記録・検証の証跡・長期保守の約束が商品**だということである。言語についても同じで、ISO 26262 の tool confidence（TCL）、IEC 61508 の tool class（T3 が code 生成 tool）、DO-178C の DO-330（tool qualification）という形で、**compiler は「資格付け」される対象**である。

この点で重要な前例が **Ferrocene**（Ferrous Systems）である。Rust compiler の downstream 配布で、TÜV SÜD により ISO 26262 ASIL D、IEC 61508 SIL 3、IEC 62304 Class C への tool qualification を取得し、`core` library の subset には ASIL B / SIL 2 の認証も付けた。資格付けの範囲は release・host・target の組ごとに限定されること、言語仕様（Ferrocene Language Specification）を固定することが資格の土台になっていること、が読み取れる教訓である。Sigil も、言語仕様の固定と版の長期保守なしに認証の土俵には上がれない。

### 4.4 形式検証という第三の道

**seL4** は別の到達点を示す。約 1 万行の microkernel について、実装が形式仕様どおりであることの機械検証済み証明を持ち、mixed-criticality 用の MCS 構成では「時間」を capability として扱う scheduling context（budget と period を持つ時間の権限）を導入した。MCS 構成の functional correctness の証明は 2026 年に RISC-V で完了したと Proofcraft が発表している。認証制度の枠内ではなく「証明」で信頼を作る道であり、Sigil の「能力（capability）の明示宣言で権限を絞る」という設計思想と最も親和性が高い先行例である。同時に、証明のコストが膨大（証明の行数が code を桁で上回る）ことも示しており、現実解は「証明済みの小さな土台 + その上で検証しやすく書ける言語」の組み合わせになる。

### 4.5 認証の重さと参入障壁の序列

業界ごとの参入障壁を、(a) 要求される integrity の最高水準、(b) 新規の言語・tool への実績要求、(c) 市場の閉じ具合、から並べると次の序列になる。

**最重**: 原子力 Category A（software 自体を避ける文化）、航空 DAL A、鉄道 SIL 4。いずれも数十年単位の実績と証跡が前提で、新規言語の最初の適用先にはならない。
**重**: 自動車 ASIL D、産業 SIL 3。ただし自動車は AI 導入の圧力で「monitor」の隙間が生まれており、産業は runtime 文化がある。
**中**: 医療 Class C（process 中心で、言語の認証という概念は薄い）、off-highway の PL d / AgPL d、宇宙（mission class による。payload 側は実験的採用の余地）。
**軽**: ロボット（規格が形成期）、船舶（型式承認型で software の階層化が弱い）、小型ドローン、家電・IoT（Class B / C 水準と security が中心）。

---

## 5. Sigil がターゲットにするには

### 5.1 言語・処理系・標準ライブラリへの要求（業界横断）

各業界の要求を Sigil への要求として翻訳すると、次のようになる。

**時間について**
- 実行時間の上限が見通せること。GC のような停止時間の読めない仕組みは安全側では受け入れられない（Sigil の設計はこの点で有利なはずだが、runtime の各機能について「時間の上限が言えるか」を監査する必要がある）。
- WCET（最悪実行時間）解析と相性のよい code 生成（間接分岐や動的 dispatch の抑制、解析可能な subset の定義）。
- 時間を資源として扱う実行 model との接続。seL4 MCS の scheduling context のように「budget と period を持つ時間の権限」を、partition の外から与えられる形に runtime が適合すること。

**メモリについて**
- メモリ使用量の上限を静的に言えること。動的確保に上限を付ける、または確保を禁止する mode を言語 subset として定義できること。
- stack 使用量の解析可能性。自動車・off-highway の小型 ECU（Cortex-M / R、AURIX 等）が最初の hardware になる可能性が高く、footprint の小ささが前提になる。

**言語仕様と subset について**
- 言語仕様の固定と版管理。認証は「どの版の言語で書かれたか」に紐付くため、仕様の安定性そのものが製品価値になる（Ferrocene の教訓）。
- MISRA のような coding 規約の言語版、すなわち「安全用途 subset」の公式定義。危険な構成要素を subset から外し、検査器で強制できること。Sigil の「保証できないことを言語の仕組みで到達不能にする」原則は、この subset の考え方と直結する。

**tool qualification と証跡について**
- compiler の資格付け package（safety manual、既知の問題の一覧、検証 test suite、要求から test への traceability）を用意できる開発体制。ISO 26262 の TCL、IEC 61508 の T3、DO-330 のどの枠で資格を取るかは業界ごとに選ぶ。
- 対象 architecture ごとの資格になることへの備え。現実的には ARM Cortex-M / R / A と RISC-V から始め、航空・鉄道で残る PowerPC や、off-highway の TriCore（AURIX）は後回しになる。
- 長期保守。10 年（医療・自動車）から 30 年（鉄道・エネルギー）まで、同じ版の compiler と runtime を保守し続ける約束が調達条件になる。

**既存の土台との接続について**
- POSIX / ARINC 653 APEX への binding と、C ABI での相互運用。Sigil で書いた component が QNX、PikeOS、INTEGRITY、VxWorks の partition としてそのまま動くことが、参入の前提条件になる。
- 認証済み土台の「外側の小さな層」として動くための runtime 形態。大きな runtime を持ち込まず、partition 内で完結する小さな実行系であること。

**security と更新について**
- SBOM の生成、署名付き build、再現可能な build。医療（FDA の cybersecurity guidance）と自動車（UN R155 / R156）が先行し、他業界にも広がっている。
- OTA 更新と認証の両立。更新可能性を前提にした partition 構成（安全側は更新しない／更新しても再認証範囲を局所化する）への適合。

### 5.2 最初の適用点の候補と、業界ごとの現実味

自動運転で挙げた「safety monitor 的 component」は、業界横断で次のように一般化できる。**AI や複雑な software の出力を、単純で検証可能な層が検査し、権限を絞って物理側へ渡す**という部品である。

| 適用点 | 業界 | 現実味 | 理由 |
|---|---|---|---|
| AI 出力の検証・権限制限（safety monitor / plausibility checker） | 自動車、ロボット、ドローン、off-highway | 高 | AI stack の新規導入が進み、認証済み部品がまだ埋めていない隙間。NVIDIA Halos の dual-channel 構成が需要の実在を示す |
| 非安全側と安全側の境界（gateway、データ検証、異常検出） | 医療、産業、電力 | 中〜高 | SOUP や Linux の「外側の層」として置く場所が制度上も明確 |
| 小型 controller の制御 software 全体 | off-highway、家電、IoT | 中 | SIL 2 / PL d / Class B 級なら認証の重さが現実的。ただし既存の C / CODESYS 生態系との競争になる |
| payload / 非安全 partition 内の software | 宇宙、防衛 | 中 | 分離が制度化されており、実験的採用の受け皿がある |
| 安全 controller そのもの（SIL 3 / ASIL D の中核） | 産業、自動車 | 低（当面） | 認証済み基盤との正面競争になり、実績の壁が高い |
| 信号・飛行制御・原子炉保護の中核 | 鉄道、航空、原子力 | 最低（長期） | DAL A / SIL 4 / Category A は実績要求が別格。地上側 tool や検証系からの参入が先 |

### 5.3 優先順位の案

以上から、Sigil の業界ターゲットは次の順序を提案する。

1. **ロボット / physical AI の safety monitor**（自動運転と地続き）。規格が形成期で、AI の安全層という部品自体が新しく、既存の認証済み基盤がまだ支配していない。Sigil の「AI エージェント向け」という出自とも一致し、設計原則（能力の明示、権限の最小化）がそのまま売りになる。
2. **off-highway（建機・農機）と産業の SIL 2 級**。認証の重さが中程度で、CODESYS のような「言語 + runtime」の文化があり、新しい言語の入り込む構造がある。小型 ECU 向けの footprint と決定性が問われる。
3. **医療の監視・境界層**。IEC 62304 は process の規格なので、言語の新規性への罰が比較的小さく、tool の妥当性確認と SOUP 管理への適合が鍵になる。
4. **宇宙の payload 側**。RTEMS / cFS のオープンな生態系と、hosted payload の分離という新しい需要がある。認証は mission class 依存で、実験の余地がある。
5. **航空・鉄道・原子力の中核は長期課題**。当面は非安全 partition、地上系、検証 tool 側から実績を積む位置づけにする。

### 5.4 P6（OS 本体）との関係

最後に、P6 の議論と混同しやすい点を切り分ける。

- **既存の認証済み土台の上で動く component を Sigil で書く**話は、OS を作る話ではない。土台（QNX、PikeOS、seL4 等）の認証は土台側が持ち、Sigil は partition 内の software の検証しやすさと言語・tool の資格付けで勝負する。これは短期〜中期に到達可能な適用点であり、本レポートの優先順位はこの前提で組んである。
- **P6 として OS 本体を Sigil で書く**話は、その OS 自体の認証という別の山になる。先行例が示す到達点の形は「小さな kernel（seL4 の約 1 万行が目安）+ 分離の構造的担保 + 可能なら証明」であり、大きな OS を書くことではない。P6 の設計では、kernel に何を入れないかの線引きと、時間・メモリを capability として扱う seL4 MCS のような model をどう取り込むかが論点になる。
- 両者は競合しない。component 言語としての実績（tool 資格、subset、traceability の仕組み）は、そのまま P6 の土台作りに転用できる。順序としては component 側が先、OS 本体はその蓄積の上に置くのが自然である。

---

## 6. 限界と注意

- 本レポートの認証に関する記述は公開情報に基づく。認証は版・target・構成ごとに発行されるため、個別の採用判断では各ベンダーの certificate と safety manual の現物確認が要る。
- 市場規模や搭載台数などの数値はベンダーの自己申告を含むため、本レポートでは判断材料から外した。
- 規格は改訂が続いている（IEC/IEEE 60880 の 2026 年改訂、ISO 10218 の 2025 年改訂、humanoid 向け ISO 25785 の策定等）。特にロボット分野は流動的で、本レポートの記述は 2026-11-16 時点のものである。

## 出典

- Avionic RTOS の概説（DO-178C、ARINC 653、VxWorks 653 / INTEGRITY-178 / LynxOS-178、FACE）: https://real-time-consulting.com/wp-content/uploads/2025/04/Avionic-RTOS.pdf
- Linux の航空安全用途への不適に関する技術論文: https://www.preprints.org/manuscript/202603.0354/v3
- separation kernel の学術 survey（ARINC 653 と各製品の認証）: https://arxiv.org/pdf/1508.07066
- FACE / MOSA の解説（防衛調達と OSS profile）: https://www.aerospace-and-defence.com/how-the-face-standard-enables-modular-open-and-future-proof-avionics-systems-a-3101d25d49b938269c13be118765d2e6
- QNX の医療向け公式資料（IEC 62304 Class C 事前認証、IEC 61508 SIL 3）: https://qnx.software/en/industries/medical-devices
- 医療機器 OS の比較と IEC 62304 / SOUP の解説: https://www.ics.com/blog/linux-os-right-platform-your-medical-device?utm_campaign=ICS%20Newsletters&utm_source=hs_email&utm_medium=email&_hsenc=p2ANqtz-9RjV332QmuKfzj5S7l1rXoI688SmZSBe78B-vm5Pwz5DSquoUpOpnsYUglSAKViaZayF8s
- SYSGO の鉄道向け公式資料（PikeOS の EN 50128 / EN 50657 SIL 4 事前認証）: https://www.sysgo.com/railway
- 鉄道 controller と SIL 4 の技術解説（SYSGO）: https://www.sysgo.com/blog/article/railway-controller-for-switching-and-crossing-a-critical-asset-in-safe-railway-signalling
- CODESYS の device 製造者向け資料（runtime の移植性、safety 拡張）: https://www.binfordtech.com.au/wp-content/uploads/2025/06/CODESYS-for-device-manufacturers-brochure-1.pdf
- 産業機械・robot の安全 AI platform 一覧（CODESYS Virtual Safe Control の SIL 3 認証、INTEGRITY、QNX の横断認証）: https://aiassurancedirectory.com/industries/industrial-machinery/
- robot の安全規格と QNX / NVIDIA Halos for Robotics の動向: https://bananarat.com/blogs/blog/qnx-robotics-humanoid-safety-layer
- RTEMS の ECSS Qualification Data Package（embedded brains）: https://embedded-brains.de/wp-content/uploads/2023/03/RTEMS_QDP_FactSheet_0323.pdf
- 衛星 software の分離と資格付けに関する考察（cFS、ECSS-Q-ST-80C）: https://www.gazagnaire.org/blog/2026-02-25-satellite-software.html
- IEC/IEEE 60880:2026 の概要（原子力 Category A software）: https://standards.iteh.ai/catalog/standards/iec/3ba1c71f-7712-429c-a8e0-fad9a32f593e/iec-ieee-60880-2026
- 原子力・航空・鉄道における hardware 分離の先例に関する論文: https://zenodo.org/records/20048098/files/P2_Software_Safety_Ceiling_ArXiv_v1.06.pdf?download=1
- TTControl TTC 2030（PXROS、SIL 2 / PL d / AgPL d / ASIL C の TÜV 認証）: https://www.ttcontrol.com/products/electronic-control-units/ttc-2030-family
- Bosch Rexroth ORC2（農機向け QNX + ROS 2、ISO 13849 / 25119 / 19014）: https://www.boschrexroth.com/en/de/company/press/orc2-34112.html
- ECDIS の DNV 型式承認証書（OS として Windows IoT 系が記載）: https://www.anschuetz.com/fileadmin/user_upload/Anschuetz_Synapsis_EcdisNX-05.XX.YY_DNV-EC-Mod-B_MEDB000058G-Rev5.pdf
- UAV autopilot の学術 survey（PX4 + NuttX、ArduPilot + ChibiOS）: https://www.mdpi.com/2079-9292/13/23/4785
- RTOS + middleware の landscape（認証型 UAS の stack、FreeRTOS / Zephyr / ThreadX の認証状況）: https://github.com/newslabntu/nano-ros/blob/HEAD/docs/research/rtos-framework-landscape-2026.md
- Eclipse ThreadX / SafeRTOS / Zephyr の認証比較: https://www.bermondseyelectronics.com/news/zephyr-vs-freertos-vs-threadx-choosing-the-right-rtos/
- Ferrocene の資格付け（TÜV SÜD、ISO 26262 ASIL D / IEC 61508 SIL 3 / IEC 62304 Class C、core subset 認証）: https://ferrocene.dev/blogs/ferrocene-26080/
- Ferrocene の資格範囲と target の限定に関する分析: https://www.tqcs.io/articles/two-routes-to-automotive-rust
- Red Hat と Ferrocene の協業（In-Vehicle OS での qualified Rust）: https://www.businesswire.com/news/home/20261013792564/en/Red-Hat-and-Ferrous-Systems-Collaborate-to-Enable-Safety-Qualified-Rust-on-Red-Hat-In-Vehicle-Operating-System
- ELISA と safety-critical Linux（Red Hat In-Vehicle OS の ASIL B SEooC 認証）: https://www.techveda.live/2026/07/30/safety-critical-linux/
- seL4 の解説（証明、capability、MCS）: https://webiano.digital/sel4-explained-the-10000-line-kernel-with-a-million-lines-of-proof/
- Proofcraft の seL4 MCS 検証完了の発表（RISC-V）: https://www.proofcraft.systems/news-2026/
- GE Vernova Mark VIe（QNX による turbine 制御）: https://www.gevernova.com/gas-power/services/gas-turbines/upgrades/mark-vie-control-system-upgrade
