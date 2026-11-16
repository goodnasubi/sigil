# 自動運転 OS 調査レポート

- 調査日: 2026-11-16
- 目的: Sigil（AI エージェント向けの安全性重視の言語）の長期方針「自動運転用にも対応したい」（2026-11-16、所有者表明）の材料とする。特に、長期段階 P6（ハードウェアの上で直接動く OS 本体、フィジカル AI 向け組み込み OS を含む。ADR 0005）の設計に入る際の論点整理を目的とする。
- 調査方法: Web 検索と一次情報（ベンダー公式資料、標準化団体の公開情報）の確認を中心とした。本レポート中の認証・採用に関する記述は、ベンダー発表または第三者認証機関への言及がある情報に基づく。ベンダーの自己申告と第三者認証は区別して記す。

---

## 1. 概要

「自動運転 OS」という単一の製品カテゴリは存在しない。実際の車両では、次の層が組み合わさって自動運転を支えている。

1. **安全側の土台**: 認証済み RTOS または separation kernel / hypervisor（QNX、INTEGRITY、PikeOS、VxWorks 等）。ASIL D の制御ループと freedom from interference（FFI、干渉からの自由）を担う。
2. **高性能計算側の基盤**: Linux 系 OS や POSIX 基盤の上で、認識・判断・経路計画などの重い処理を動かす（NVIDIA DRIVE OS、EB corbos Linux、AGL、AUTOSAR Adaptive Platform の実装基盤等）。
3. **middleware / framework 層**: ROS 2、AUTOSAR Adaptive の ARA、Apex.OS、Apollo Cyber RT 等。自動運転 stack の部品間通信と実行管理を担い、OS そのものではない。
4. **cockpit / infotainment 側**: Android Automotive OS や AGL が担う領域。安全クリティカルな制御とは分離して hypervisor 上の guest として動くのが通例。

自動運転の量産では「1 つの OS で全部」ではなく、**mixed criticality（安全度レベルの異なる処理の混載）を、認証済みの小さな土台で分離する**構成が主流である。この構造そのものが、Sigil の P6 を考えるときの前提になる。

主要な発見は次のとおり。

- 安全側の土台は QNX OS for Safety、INTEGRITY、PikeOS、VxWorks Cert Edition など、いずれも **ISO 26262 ASIL D の事前認証（pre-certified）を持つ小さな kernel** が占める。認証済みであること自体が製品価値であり、車両全体の認証コストと期間を下げる売り文句になっている。
- Linux は高性能計算側では事実上の標準だが、安全用途では「そのまま」ではなく、hypervisor による外部監視（EB corbos Linux for Safety Applications、ASIL B / SIL 2 の技術評価）や、AGL のように機能安全対応を計画・推進する形で入ってきている。Linux 単体で ASIL D の中核制御を担う例は確認できなかった。
- 規格は ISO 26262（故障への対応）だけでは完結しない。ISO 21448（SOTIF、性能限界への対応）、ISO/SAE 21434 と UN R155（cybersecurity）、UN R156（software update の型式認証要件）が重なり、OS には「安全」と「security」と「更新可能性」の同時成立が求められる。
- 業界は SDV（software-defined vehicle）と中央集約・zonal architecture へ移行中であり、OS / middleware 層の共通化・オープンソース化（SOAFEE、Eclipse SDV の S-CORE、AGL）が加速している。ただしオープンソース側の安全認証はまだ ASIL B 相当が中心で、ASIL D の中核は商用認証済み kernel が担い続けている。
- Sigil の設計原則（能力の明示宣言、保証できないことを言語の仕組みで到達不能にする、信頼基盤を小さく保つ）は、この領域の要求（FFI、最小の信頼基盤、監査可能性）と強く重なる。一方で、リアルタイム性の保証、認証プロセスへの適合、AI / ML ワークロードとの境界が大きな課題になる（第 6 章）。

---

## 2. 主要な自動運転向け OS / プラットフォーム

### 2.1 一覧表

| 名称 | 提供者 | 位置づけ | 安全認証 | 主な採用・用途 |
|---|---|---|---|---|
| QNX OS for Safety | BlackBerry QNX | microkernel RTOS。QNX Hypervisor for Safety も提供 | TÜV Rheinland により ISO 26262 ASIL D 認証。IEC 61508 SIL 3 も事前認証 | 2 億 7,500 万台超の車両に搭載（ベンダー公表）。BMW、Bosch、Continental、Geely、Honda、Mercedes-Benz、Toyota、Volkswagen、Volvo 等が採用。Momenta + XHEART の自動運転 platform の基盤にも採用 |
| NVIDIA DRIVE OS / Halos OS | NVIDIA | Linux 基盤の車載 OS を含む DRIVE platform の system software。Halos は full-stack の安全体系 | DriveOS 6.0 が TÜV SÜD により ISO 26262 ASIL D 適合認証。NVIDIA の開発 process は ISO/SAE 21434 の process 認証を取得 | DRIVE Hyperion / DRIVE AGX を使う OEM（Volvo が次世代車で Hyperion + Halos + Alpamayo の採用を 2026-10 に発表） |
| AUTOSAR Adaptive Platform | AUTOSAR partnership | OS ではなく標準仕様。POSIX（PSE51）OS 上の runtime（ARA）と functional cluster 群を規定 | 仕様自体は認証対象ではない。実装（各社の platform 製品）が認証を受ける | BMW、Bosch、Continental、Mercedes-Benz、Volkswagen 等が設立 partner。Vector、ETAS、Elektrobit 等が実装を提供。中央 computer、自動運転、OTA 管理が主用途 |
| Automotive Grade Linux (AGL) | Linux Foundation | Yocto 基盤のオープンソース Linux distribution（Unified Code Base） | 機能安全（ISO 26262）対応は community で計画段階。現状は認証済み platform ではない | 2018 年型 Toyota Camry 以降、Toyota / Lexus の infotainment で量産採用。instrument cluster、telematics 等へ拡大。SDV 向け reference platform「SoDeV」を推進 |
| ROS 2 / Autoware | Open Robotics / Autoware Foundation（Tier IV が中心） | middleware（ROS 2）と、その上のオープンソース自動運転 stack（Autoware）。OS ではない | ROS 2 本体に安全認証はない。量産では認証済み基盤との組み合わせが前提 | 研究・開発の事実上の標準。Tier IV が robotaxi、自動運転 bus 等で商用展開 |
| Baidu Apollo / Cyber RT | Baidu | オープンな自動運転 platform と独自 middleware（Cyber RT）。Linux + RT 拡張の上で動作 | Apollo software framework が 2025-01 に機能安全 ASIL D 認証を取得したとされる（Baidu 系資料） | 中国で robotaxi（Apollo Go）を大規模展開。Open Platform 11.0（2026-01） |
| Apex.OS | Apex.AI | ROS 2 API 互換の framework / middleware（Apex.Grace）と通信 middleware（Apex.Ida）。OS 本体ではなく、Linux や RTOS の上で動く | Apex.Grace が ISO 26262 ASIL D の安全認証を取得（TÜV Nord 系の認証として公表） | 自動車、trucking、robotics、農業・産業機械。Volvo Cars が出資・提携したことでも知られる |
| INTEGRITY | Green Hills Software | separation kernel 型 RTOS。Multivisor が secure virtualization を提供 | ISO 26262 ASIL D 認証、IEC 61508 SIL 4。security では Common Criteria EAL 6+（SKPP）の実績 | 航空・防衛・医療でも実績。自動車では NXP と組んで ADAS / 自動運転向け platform を提供。数億台規模の車両 ECU で稼働（ベンダー公表） |
| VxWorks（Cert Edition） | Wind River（Aptiv 傘下） | 商用 RTOS。Helix Virtualization Platform が mixed criticality の統合基盤 | VxWorks Cert Edition が TÜV SÜD の証明書付きで ISO 26262 ASIL D の certification evidence を提供 | 航空宇宙・防衛・産業で広い実績（20 億台超の device 展開はベンダー公表）。自動車では safety island や domain controller の用途 |
| PikeOS | SYSGO（Thales グループ） | separation kernel 型 RTOS 兼 type-1 hypervisor。L4 系譜の microkernel | ISO 26262 は ASIL D まで certifiable な certification kit を提供。DO-178C DAL A、IEC 61508 SIL 4、Common Criteria EAL 5+ の実績 | 航空・鉄道・防衛が主戦場。自動車では AUTOSAR partition や secure connectivity platform 等で利用 |
| EB corbos Linux for Safety Applications | Elektrobit（Continental 傘下） | Ubuntu 基盤の Linux distribution に、hypervisor 上の外部監視 component を組み合わせた安全対応 Linux | TÜV Nord から ISO 26262 ASIL B / IEC 61508 SIL 2 の positive technical assessment（SEooC として）。「機能安全規格への適合を評価された初のオープンソース OS solution」と公表 | Mobileye Drive（Level 4 system）への統合が 2026-02 に発表。ETAS の platform との統合 stack も提供 |
| Android Automotive OS (AAOS) | Google | Android 基盤の車載 OS。infotainment が主対象で、車両機能の一部（空調等）も扱う | 自動運転の安全中核を担う認証は持たない。安全側とは hypervisor で分離するのが前提 | Volvo、Renault、Ford、GM 等が Google Automotive Services 付きで採用。「Google built-in」として展開 |

### 2.2 安全側の土台: 認証済み RTOS / separation kernel

**QNX OS for Safety** は現在の量産車における安全側 OS の代表格である。microkernel 構成で、driver や protocol stack を kernel の外の process として動かし、障害の影響範囲を局所化する。QNX Hypervisor for Safety も含め TÜV Rheinland による ISO 26262 ASIL D 認証を受けており、OEM 側は「認証済みの部品を使うことで、自社が認証すべき範囲を安全クリティカルな部分だけに絞れる」ことを利点として挙げる。2026 年には Momenta と XHEART が QNX OS for Safety（QNX SDP 8.0 基盤）を自動運転 platform の中核に選び、ISO 26262 ASIL D 認証済みの量産向け solution として発表している。また QNX は ISO/SAE 21434 への適合（cybersecurity assurance）も併せて訴求している。

**INTEGRITY**（Green Hills Software）は separation kernel の代表で、時間と空間の両方で partition を分離し、各 application に CPU 時間と memory 資源を保証する設計を特徴とする。ISO 26262 ASIL D と IEC 61508 SIL 4 の安全認証に加え、Common Criteria EAL 6+ という security 側の高い評価実績を持つ点が他と異なる。Multivisor により、認証済み application と Linux / Android の guest OS を同一 SoC 上で FFI を保ちながら混載できる。自動運転向けには「AI の black box な推論 engine と並行して、ASIL D の plausibility analysis（妥当性検査）を INTEGRITY 側で走らせる dual-channel 構成」が提案されている。これは第 6 章の Sigil への示唆でも重要になる。

**PikeOS**（SYSGO）は航空分野の ARINC 653 partitioning の系譜を持つ separation kernel 兼 hypervisor で、partition ごとに guest OS（Linux、AUTOSAR、POSIX personality 等）を載せられる。ISO 26262 ASIL D までの認証に対応する certification kit（証跡 package）を提供し、航空（DO-178C DAL A）や鉄道（EN 50128 SIL 4）での認証実績を自動車へ転用する位置づけである。

**VxWorks**（Wind River）は航空宇宙・防衛・産業で最も広く使われてきた商用 RTOS の一つで、Cert Edition が TÜV SÜD の証明書とともに ISO 26262 ASIL D の certification evidence package を提供する。Helix Virtualization Platform は VxWorks と Wind River Linux を統合し、mixed criticality の consolidation を狙う。自動車単独での大規模採用というより、他産業での認証実績と長期保守（lifecycle support）を強みとする。

この 4 者に共通するのは、**(a) kernel が小さい、(b) partition 分離で FFI を構造的に担保する、(c) 認証の証跡（safety manual、証明書、tool 資格）まで含めて製品化されている**、という点である。

### 2.3 高性能計算側: platform OS と標準仕様

**NVIDIA DRIVE OS** は、DRIVE AGX SoC 上で動く車載 OS 群であり、2025 年以降は安全体系全体を **NVIDIA Halos** と呼ぶ構成に再編された。Halos OS は ASIL D 認証済みの DriveOS を土台に、Halos Core（safety OS）、SDK（safety middleware）、application の 3 層で構成される。DriveOS 6.0 は TÜV SÜD により ISO 26262 ASIL D 適合の認証を受け、Thor 世代の SoC も ASIL D 適合の評価を受けている。Hyperion は sensor と dual SoC の冗長構成を持つ reference architecture で、Level 2++ から Level 4 までを OTA で拡張する設計を採る。Volvo が次世代量産車に Hyperion + Halos の採用を決めたことは、「SoC + OS + 安全体系」を一括で買う platform 型の調達が進んでいることの表れである。

**AUTOSAR Adaptive Platform** は OS そのものではなく、POSIX OS の上で動く software platform の標準仕様である。Classic Platform（OSEK 基盤、静的構成、C 言語、microcontroller 向け）と対をなし、Adaptive は service-oriented architecture（SOA）、動的な service discovery、SOME/IP や DDS による通信、C++、OTA による更新を前提とする。実行管理（ara::exec）、通信管理（ara::com）、診断、persistency、cryptography、identity and access management 等の functional cluster が標準化されている。自動運転の中央 computer はほぼこの Adaptive の想定する姿（高性能 SoC + POSIX OS）で設計されており、実装は Vector、ETAS、Elektrobit 等の supplier が提供する。

**EB corbos Linux for Safety Applications**（Elektrobit）は、「Linux を安全用途で使いたい」という需要への現時点での代表的な回答である。Ubuntu 基盤の Linux に対し、hypervisor 環境に置いた外部監視 component が kernel の動作を intercept・検証することで安全を担保し、TÜV Nord から ASIL B / SIL 2 の technical assessment を SEooC（Safety Element out of Context）として受けた。ASIL D ではなく ASIL B である点に注意が要るが、Level 4 system である Mobileye Drive への採用（2026-02 発表）は、オープンソース基盤の Linux が自動運転の量産 stack に入り始めた象徴的な事例である。

### 2.4 middleware / framework 層

**ROS 2** は研究開発の標準 middleware であり、**Autoware**（Autoware Foundation、中心は Tier IV）はその上のオープンソース自動運転 stack である。sensing、localization、perception、planning、control、vehicle interface を一通り備え、Level 4 を目標とする。ただし ROS 2 自体に機能安全認証はなく、量産適用では安全監視や fallback を別系統で用意するのが通例である。

**Apex.OS**（Apex.AI）はこの弱点への回答で、ROS 2 の API を保ちながら real-time 性と決定性を強化した framework（Apex.Grace）と、shared memory 基盤の通信 middleware（Apex.Ida）から成り、Apex.Grace は ISO 26262 ASIL D の安全認証を取得している。「開発者は ROS 2 の作法のまま、認証済みの土台に移れる」ことが価値提案である。

**Baidu Apollo** は中国における量産・運行の代表で、独自 middleware の Cyber RT（user-space coroutine による低 latency scheduling、zero-copy 通信）を中核に、Open Platform として公開されている。Apollo Go の robotaxi 運行規模は世界最大級であり、「認証済み OS」よりも「運行実績と data の蓄積」で安全性を積み上げるアプローチの代表例でもある。

### 2.5 cockpit 側

**Android Automotive OS** は Google の Android を車載化したもので、infotainment と一部の車両制御 UI を担う。自動運転の安全中核ではないが、SDV では cockpit domain と自動運転 domain が同じ SoC / hypervisor 上に同居するため、「安全でない側をどう隔離するか」という観点で自動運転 OS の設計に直接関わる。AGL も元々は infotainment の共通基盤として始まり、cluster、telematics、さらに SDV 全体の reference へ守備範囲を広げている。

---

## 3. 安全規格と認証の要件

自動運転 OS に関わる規格・規制は、少なくとも次の 5 系統がある。OS や基盤 software への要求は、これらの重なりとして決まる。

### 3.1 ISO 26262（機能安全）

電気・電子（E/E）system の**故障**による hazard を扱う自動車機能安全の中心規格（2018 年版が現行。第 3 版の改訂作業が進行中とされる）。Hazard Analysis and Risk Assessment（HARA）で Severity、Exposure、Controllability から ASIL（QM、A、B、C、D）を定め、ASIL が高いほど開発・検証の厳格さが増す。自動運転では「driver が介入できない」ため Controllability が最悪側に倒れやすく、steering、braking、自動運転の制御 chain は ASIL D になりやすい。

OS に対する具体的な要求として重要なのは次である。

- **Freedom from interference**: 安全度の異なる software 要素を同一 ECU に混載する場合、低 ASIL 側の故障が高 ASIL 側へ波及しないことを示す必要がある。これが separation kernel / hypervisor と partitioning OS の存在理由になっている（INTEGRITY や PikeOS の資料でも、ISO 26262 が partitioning を cascading failure 防止の設計手段として挙げていることが明示される）。
- **事前認証部品の利用**: OS が SEooC として認証済みであれば、車両側の認証は統合と usage の正しさに集中できる。QNX や VxWorks Cert Edition が「pre-certified」を強く訴求するのはこのため。
- **tool の資格（tool qualification）**: compiler や検証 tool も信頼性分類（TCL）の対象になる。言語処理系を作る Sigil にとっては、OS 本体だけでなく toolchain 全体が認証の射程に入る点が重要である（第 6 章）。

### 3.2 ISO 21448（SOTIF: Safety of the Intended Functionality）

故障がなくても起きる危険、すなわち**意図した機能の性能限界**による hazard を扱う（2022 年発行）。逆光で camera が歩行者を認識できない、といった perception の限界が典型例である。既知・未知 × 安全・危険の 4 象限で scenario を管理し、未知の危険 scenario を simulation、test、運行 data から既知化し、対策済みに移していく活動を求める。

OS / 基盤 software の観点では、SOTIF は「OS の認証」という形にはなりにくいが、(a) scenario の再現に必要な record / playback の決定性、(b) 運行中の monitoring と data 収集、(c) 性能限界に陥ったときの fallback（minimal risk maneuver 等）への移行保証、といった形で基盤の要件になる。

### 3.3 ISO/SAE 21434 と UN R155（cybersecurity）

ISO/SAE 21434（2021）は車両の lifecycle 全体にわたる cybersecurity engineering の規格で、Threat Analysis and Risk Assessment（TARA）を中核とする。UN Regulation No. 155 はこれを型式認証の要件にしたもので、OEM に Cyber Security Management System（CSMS）の認証取得を求める。EU では 2022 年 7 月から新 vehicle type、2024 年 7 月から全ての新車に適用されており、日本も含む 1958 年協定の締約国で型式認証の条件になっている。CSMS 証明書の有効期間は最長 3 年である。

OS への影響は直接的である。secure boot、鍵管理、intrusion detection、脆弱性対応の process、supplier への要求の flow-down が必要になり、QNX や NVIDIA が ISO/SAE 21434 の適合・process 認証を OS の売りの一つにしているのはこのためである。

### 3.4 UN R156（software update）と OTA

UN Regulation No. 156 は Software Update Management System（SUMS）を型式認証の要件とし、OTA を含む software update について、update の真正性・完全性の保護、失敗時の復旧または安全状態への移行、型式認証に関わる software の識別子（RXSWIN）の監査可能な管理を求める。工程側の標準として ISO 24089 がある。

つまり SDV の「出荷後も更新し続ける」という前提は、規制上も管理された形でしか許されない。OS には、**A/B partition や rollback を含む安全な更新機構、更新の署名検証、更新中・失敗時の安全状態の保証**が基盤機能として求められる。

### 3.5 UL 4600 と、その他の関連規格

**ANSI/UL 4600**（第 3 版、2023）は、driver の監督を前提としない自律 product のための safety case（安全の論証）の規格である。処方的な設計規則ではなく「安全であることの主張と証拠をどう構成するか」を定める点が特徴で、ISO 26262 や SOTIF の結果を 1 つの safety case に束ねる役割を果たす。robotaxi 事業者や ADS 開発者が主な利用者である。

このほか、AI の安全に特化した **ISO/PAS 8800**（AI を含む車両機能の安全）、Level 3 / 4 ADS の設計・検証・運用後までを扱う **ISO/TS 5083**（2025、旧 ISO/TR 4804 の後継）が整備されつつある。自動運転の型式認証では UN R171（DCAS、driver control assistance system）など自動運転機能そのものに関する規則も進んでおり、OS 単体の認証だけで完結する話ではない。

### 3.6 小括: OS に求められることの重なり

| 観点 | 主な根拠 | OS / 基盤への要求 |
|---|---|---|
| 故障への安全 | ISO 26262 | ASIL に応じた開発・検証、FFI の構造的担保、診断と安全状態への移行、tool 資格 |
| 性能限界への安全 | ISO 21448（SOTIF） | 決定的な record / replay、運行 monitoring、fallback への移行保証 |
| 悪意への安全 | ISO/SAE 21434、UN R155 | secure boot、鍵・証明書管理、侵入検知、脆弱性対応 process、supplier 管理 |
| 更新の安全 | UN R156、ISO 24089 | 署名検証、rollback、安全な更新手続き、版の識別と監査証跡 |
| 安全の論証 | UL 4600 | safety case を構成できる証跡（log、trace、検証結果）の生成と保持 |

---

## 4. 技術要件

### 4.1 リアルタイム性と時間の決定性（determinism）

自動運転の制御 loop（認識→判断→制御）は millisecond 単位の周期で回り、要求されるのは「速いこと」ではなく「**最悪応答時間が見積もれること**」である。brake や steering の制御では、平均 latency が小さくても稀に大きく跳ねる system は使えない。このため安全側では priority-based preemptive scheduling を持つ RTOS が使われ、worst-case execution time（WCET）の見積りと scheduling 解析が認証の一部になる。

Linux は PREEMPT_RT により firm real-time 相当の用途が広がったが、kernel の規模と複雑さのため、最悪値の保証と認証は RTOS と比べて難しい。量産構成で「認識・計画は Linux 側、最終的な制御と安全監視は RTOS 側」という分担が多いのはこのためである。

もう一つの決定性は**再現性**である。事故・ヒヤリハットの解析や SOTIF の scenario 管理では、記録した sensor data を再生して同じ挙動を再現できることが求められる。Apex.OS の V&V 機能（time- and data-deterministic な record / playback の訴求）や、Autoware の simulation 環境（AWSIM 等）はこの要求に応えるものである。

### 4.2 mixed criticality の分離

中央集約化が進むほど、ASIL D の制御、ASIL B の監視、QM の infotainment が同じ SoC に同居する。分離の手段は主に次の 3 層である。

- **時間の分離**: partition ごとに CPU 時間の配分を保証し、低 criticality 側の暴走が高 criticality 側の周期を奪わない（INTEGRITY、PikeOS の time partitioning）。
- **空間の分離**: MMU / MPU による memory 保護と、partition 間の通信路の限定（separation kernel の設計原則: 迂回不能、改竄不能、検証可能性）。
- **資源の分離**: device や I/O の割当てを静的に定め、共有 device は VirtIO 等の標準化された経路で仲介する（QNX Hypervisor for Safety の safety-certified VirtIO driver 等）。

この分離を「運用の注意」ではなく kernel の構造で保証することが、認証済み土台の共通点である。

### 4.3 fault tolerance と冗長化

Level 3 と Level 4 以上では、fallback の責任の置き場が異なる。Level 3 は、介入要求や故障への応答を担う fallback-ready user の存在を前提とする（SAE J3016）。system は引き継ぎが成立するまでの間、安全を保ち続けなければならない。一方、人の介入を期待せず system 自身が fallback を担うのは Level 4/5 で、この場合は fail-operational に近い要求が全面に出る。引き継ぎ条件を前提にできるか、自動 fallback が必須かで、冗長化の要求も変わる。代表的な手段は次である。

- **冗長な計算経路**: NVIDIA Hyperion の dual SoC 構成のように、同じ入力を 2 系統で処理し、異常時に切り替える。ASIL 分解（ASIL decomposition）の考え方で、冗長な低 ASIL 要素の組み合わせにより高 ASIL を達成する設計も ISO 26262 の枠内にある。
- **safety monitor / plausibility check**: AI の推論結果を、単純で検証可能な rule-based な検査で監視する dual-channel 構成（Green Hills の提案等）。複雑な stack 全体を認証する代わりに、監視側を小さく認証可能に保つ。
- **lockstep、ECC、watchdog、電源・clock の監視**といった hardware 機構との連携。OS はこれらの診断結果を受けて、安全状態への移行（degraded mode、minimal risk maneuver）を実行する役割を負う。

### 4.4 OTA 更新

UN R156 の下で、更新は「機能」であると同時に「認証対象の変更」である。基盤には次が求められる。

- 署名検証と secure boot chain との一貫性
- 更新失敗時の rollback、または安全状態の維持
- 更新対象の版管理（RXSWIN）と監査証跡
- 安全関連 update の実行条件（車両状態、電源の十分性、利用者への通知）

AUTOSAR Adaptive が Update and Configuration Management を functional cluster として持つことや、AGL / Eclipse SDV が OTA を中核機能に据えることは、この要求の表れである。

### 4.5 起動時間

車両では「電源投入から制御可能になるまでの時間」が安全と商品性の両方に効く。後退 camera の表示義務のような規制要件もあり、cockpit 側にも高速起動が求められる。RTOS は一般に起動が速く、Linux 側は起動の段階化（まず安全機能と表示、後から rich な機能）で対応する。中央 computer の統合が進むほど、「どの partition から先に起動し、依存関係をどう満たすか」という boot sequencing が OS の設計論点になる。

### 4.6 機能安全と security の両立

safety と security はときに競合する。例: security のための暗号化・認証が latency と決定性を悪化させる、safety のための単純化が security 機能の搭載を妨げる、更新による security 修正が認証済み構成を変えてしまう。UN R155 / R156 が型式認証に組み込まれたことで、この両立は設計の好みではなく市場参入の条件になった。INTEGRITY が safety（ASIL D）と security（EAL 6+）の両方の認証実績を前面に出すこと、QNX や NVIDIA が ISO/SAE 21434 を OS の訴求点に入れることは、この市場構造を反映している。

---

## 5. 業界動向

### 5.1 SDV と中央集約・zonal architecture

車両の E/E architecture は、機能ごとの ECU が多数分散する構成から、domain controller を経て、**少数の中央 computer + 物理配置ごとの zonal controller** へ移行している。調査会社 IoT Analytics の 2026 年の SDV 採用調査では、OEM の 90% 超が zonal architecture への移行に commitment し、80% が移行中とされる。McKinsey は自動車 software / electronics 市場が 2035 年に約 5,190 億ドル規模へ成長し、OS と middleware の成長率が特に高いと見ている。

この移行は OS の要件を変える。ECU ごとに閉じていた時代は「小さい RTOS が大量に」で足りたが、中央 computer では「**高性能 SoC の上で、異なる安全度と異なる更新頻度を持つ workload を、長期間にわたって運用・更新する platform**」が必要になる。Volkswagen グループと Rivian の合弁（zonal architecture の共同開発）、Stellantis の platform 統合、Ford の中央集約 + Level 3 計画（2028 年）など、OEM の動きはこの方向で一致している。

### 5.2 オープンソース化の動き

非差別化領域（差別化に直結しない基盤 software）を共同開発し、認証まで含めて共有しようという動きが強まっている。

- **SOAFEE**（Scalable Open Architecture for Embedded Edge）: Arm が主導し 150 超の partner が参加。cloud-native な開発手法（container、CI/CD、仮想 platform）を車載へ持ち込み、cloud と車両で同じ workload を動かす environmental parity を掲げる。参考実装として EWAOL（Yocto 基盤）や Open AD Kit（Autoware 基盤）がある。
- **Eclipse SDV / Eclipse S-CORE**: Eclipse Foundation の SDV Working Group の下で、自動車 grade のオープンソース software stack「S-CORE」（Safe Open Vehicle Core）を共同開発する。2025-06 に VDA と Eclipse が交わした MoU は参加企業が 11 社から 32 社へ拡大し（BMW、Mercedes-Benz、Volkswagen、Bosch、ETAS、Elektrobit、QNX 等）、S-CORE は 2025-11 に初の公開 release（0.5）を出した。開発 process は ASPICE、ISO 26262、ISO/SAE 21434 への適合を外部監査付きで進めるとされ、2026 年の full release と 2030 年までの量産適用が目標とされる。
- **AGL**: Toyota を中心に量産実績を持ち、SDV 向けの「SoDeV」platform で守備範囲の拡大を図る。機能安全対応も community の課題として挙がっている。

ただし現時点では、オープンソース側の安全水準は ASIL B 相当（EB corbos Linux の技術評価、S-CORE の目標水準）が中心であり、ASIL D の中核制御は商用認証済み kernel が担う構図は変わっていない。「オープンな基盤の上で、認証済みの小さい安全核が最終防衛線になる」という二層構造が、当面の着地点になりそうである。

### 5.3 中国の動向

中国では Baidu Apollo の robotaxi 運行に代表される「運行規模で安全性を実証する」路線と、Huawei や各 OEM の中央集約 platform が並走している。規制面でも、中国は車両 cybersecurity と software update の国家標準（GB 44495-2024、GB 44496-2024）を 2026-01 から新 vehicle type に必須化しており、UN R155 / R156 とは別系統の認証対応が必要になっている。自動運転 OS を「世界市場で売れる基盤」として考えるなら、欧州（UN 規則）、中国（GB 標準）、米国（州ごとの許可と NHTSA の枠組み）の 3 つの認証・許可 regime の差異が前提になる。

### 5.4 AI 化の進展と規格の追随

end-to-end の AI model による運転（NVIDIA Alpamayo のような reasoning model、Tesla の FSD 等）が広がるにつれ、「検証可能な部品の集合」としての従来の安全論証は難しくなっている。NVIDIA Halos が AI model に対する独立した safety check と inspection lab（ISO/IEC 17020 の認定を受けた AI Systems Inspection Lab）を組み合わせていることや、ISO/PAS 8800（AI 安全）、ISO/TS 5083 の整備は、この変化への規格側の応答である。OS の役割も「処理を正しく実行する」から「**AI の出力を監視し、根拠を記録し、必要なら権限を絞る**」方向へ広がっている。

---

## 6. Sigil への示唆

Sigil の設計原則は ADR 0027（AI が書く前提の安全設計原則）に定められている。要点は、(1) 情報フロー（汚染）の型レベル追跡、(2) ケイパビリティ指向を既定とし ambient authority を排除する、(3) 危険なデフォルトを言語と標準ライブラリから排除する、(4) 機械可読で高速なフィードバックを言語仕様の一部とする、である。また ADR 0004 の効果（effects）宣言により「このコードが到達できる範囲」を宣言として監査可能にすること、信頼基盤（trust base）を小さく・明示的に保つことが言語の中心的な約束になっている。これらを自動運転 OS の要件に重ねると、次の対応と課題が見える。

### 6.1 重なるところ

**a) 「保証できないことは、最初からできないようにする」は FFI の言語版である。**
ISO 26262 が mixed criticality の統合で求める freedom from interference は、現状では separation kernel の partition と運用規律で担保されている。Sigil の効果宣言とケイパビリティは、これを**言語の型のレベルで**表明し検査できる可能性を持つ。たとえば「この module は actuator への出力能力を持つが、network への送信能力は持たない」といった到達範囲の限定は、partition 設計の意図を code そのものに埋め込み、監査可能にする。partition の境界を越える通信が能力として宣言されていなければ書けない、という性質は、FFI の論証資料（証跡）としても意味を持つ。

**b) 信頼基盤を小さく保つ姿勢は、認証済み土台の設計と同型である。**
QNX、INTEGRITY、PikeOS の競争力の源泉は「小さい kernel を認証し、大きい機能はその外に置く」ことにある。Sigil が trust base を小さく・列挙可能に保つ原則は、この構造と言語側から一致する。P6 で Sigil が OS 本体を対象にする場合も、「認証すべき核はどこまでか」を言語の信頼基盤の宣言と対応づけられることが強みになりうる。

**c) 危険なデフォルトの排除は、安全文化の要求と一致する。**
fail-open な挙動、検証なしの通信、デフォルト認証情報といった「慣習に埋もれた危険」は、自動車の安全審査でも繰り返し問題になる。Sigil の「安全な書き方が最短の書き方」という原則は、AI が code を書く前提では特に効く。自動運転の文脈では書き手が AI エージェントである度合いが今後さらに上がるため、この原則の価値は相対的に高まる。

**d) 機械可読な診断と決定的な build は、認証の証跡生成と相性が良い。**
ISO 26262 の認証は、code そのものと同じくらい「証跡の生成可能性」で決まる。再現可能な build、構造化された診断、宣言として読み取れる到達範囲は、safety case（UL 4600）の材料を機械的に集める土台になる。DARPA AIxCC で示された「再現可能な検証の loop が自律 system の安全を押し上げる」という知見とも方向が一致する。

### 6.2 課題になるところ

**a) リアルタイム性の保証が言語の現在の射程にない。**
自動運転 OS の中核要件は「時間内に終わることの保証」だが、Sigil の効果宣言は「何に到達できるか」であって「いつまでに終わるか」ではない。WCET の見積り可能性、割込み latency、scheduling 可能性の解析とどう接続するか（例: 時間予算の宣言と静的検査、実行時間の上限を持つ runtime 構造）は、P6・組み込みの段で避けて通れない論点になる。ADR 0029 で導入が決まった arena 的な確保の議論（一時値の扱い、上限超過時の停止）も、この時間・資源の上限管理と地続きである。

**b) 認証は言語機能ではなく process と証跡の問題である。**
ASIL D の認証では、言語仕様の正しさだけでなく、開発 process、検証の網羅性、tool qualification（compiler が TCL の対象になる）が問われる。Sigil の toolchain（検査器、評価器、native 出力）が認証の射程に入ることを前提に、「どの部分を認証対象とし、どの部分は認証済み土台（既存 RTOS 等）に委ねるか」の線引きが要る。現実解はおそらく段階的で、まず (i) 認証済み RTOS / hypervisor の上で動く Sigil 製 application・監視 component、次に (ii) safety monitor のような小さく検証可能な部品、最後に (iii) OS 本体、という順序になる。

**c) AI / ML workload との境界をどう引くか。**
認識・判断の中核は当面、学習済み model と GPU / NPU 上の推論であり、Sigil が現在持たない能力（数学ライブラリ、tensor、自動微分、SIMD / GPU）にも依存する。自動運転への対応は「運転 stack 全体を Sigil で書く」ことと同義ではなく、現実的には **AI の出力を検証し、権限を制御し、安全側へ倒す component を Sigil で書く**形（INTEGRITY の dual-channel 提案と同型）が最初の適用点になる。このとき、推論結果の「汚染」や信頼度を型で扱う ADR 0027 決定 1（情報フローの型レベル追跡）の設計が、perception 出力の扱いにどう効くかは具体的な論点になる。

**d) 更新（OTA）と認証の緊張を言語側でどう扱うか。**
SDV では出荷後の更新が前提だが、更新は認証済み構成を変える。UN R156 の求める版管理・rollback・監査証跡は OS の機能であると同時に、「更新された code が宣言した能力の範囲を変えていないか」を機械的に検査できるか、という言語側の問いでもある。効果宣言の diff を更新審査の入力にする、という発想は Sigil ならではの貢献になりうるが、仕様化はこれからである。

**e) 既存資産との相互運用。**
量産の現場は AUTOSAR（Classic / Adaptive）、ROS 2、DDS / SOME/IP の上に立っている。Sigil が自動運転で使われるには、これらの境界（IDL、service 定義、通信 middleware）との接続をどう設計するかが実務上の入口になる。P6 の前に、P1 世代の stdlib 拡張（通信、暗号、認証は既に進行中）との連続性として検討できる。

### 6.3 P6・組み込みの段で論点になりそうなこと（候補一覧）

1. 時間の扱い: 周期実行、deadline、時間予算を言語の宣言（効果の拡張）として表すか、runtime の構造で固定するか。
2. 分離の単位: Sigil の module / 能力境界と、separation kernel の partition 境界をどう対応づけるか。能力宣言から partition 構成を生成・検証できるか。
3. 安全監視 component の reference 実装: 認証済み RTOS 上で動く plausibility checker を Sigil で書く小さな実験（P6 着手前の前倒し実験として）。
4. 認証戦略: toolchain の tool qualification を見据えた、検査器・code 生成の検証可能性の設計。認証対象を絞るための trust base の線引き。
5. record / replay の決定性: SOTIF 対応に要る決定的な再実行を、言語の決定性（決定的 formatter、再現可能 build の延長）としてどこまで保証するか。
6. 更新の検査: OTA 更新時に効果宣言の差分を審査する仕組みを、UN R156 の SUMS process と接続する形。
7. AI 出力の型扱い: perception / 推論の出力を「信頼できない入力」の一種として taint 設計に含めるか。minimal risk maneuver への移行条件を能力として宣言するか。

---

## 出典

プラットフォーム・ベンダー資料:

- QNX for Automotive（業界 brief、QNX OS for Safety / Hypervisor for Safety の ASIL D 認証、ISO/SAE 21434 への言及）: https://qnx.software/content/dam/qnx-xwalk/pdf/industry-briefs/qnx-automotive-solutions-short.pdf
- QNX OS for Safety 2.2.3（AWS Marketplace、認証一覧）: https://aws.amazon.com/marketplace/pp/prodview-26pvihq76slfa
- Momenta / XHEART / QNX の自動運転 platform 発表（ASIL D 認証、UN R171 への言及）: https://www.stocktitan.net/news/BB/momenta-xheart-and-qnx-build-an-autonomous-driving-platform-rawzhstf8bob.html
- QNX と neueHCT の German OEM 向け program 採択（採用 OEM の列挙、搭載台数）: https://www.centralcharts.com/en/811-blackberry/news/5787260-qnx-and-neuehct-join-forces-to-accelerate-global-intelligent-driving-deployment-anchored-by-major-german-automaker-program-win
- NVIDIA Halos（Autonomous Vehicles、DriveOS 6.0 の TÜV SÜD ASIL D 認証、ISO/SAE 21434 process 認証）: https://www.nvidia.com/en-sg/solutions/autonomous-vehicles/safety/
- NVIDIA Halos 発表 blog: https://blogs.nvidia.com/blog/halos-safety-system-autonomous-vehicles/
- AUTOSAR Adaptive Platform（公式、ARA と POSIX PSE51 の説明）: http://www.autosar.org/standards/adaptive-platform/page-2
- AUTOSAR Classic / Adaptive の比較解説: https://agnile.com/blog/autosar-classic-vs-adaptive
- AGL Japan Community 資料（Toyota Camry 以降の量産採用、SoDeV、機能安全の計画）: https://hosted-files.sched.co/aglammjapan2026/ca/AGLAMM_2026_Japan_Hiroyuki_Ishii.pdf
- Wind River: AGL の紹介（member 構成）: https://www.windriver.com/resource/accelerating-time-to-market-with-open-source-for-car-systems
- Apex.AI の製品構成（Apex.Grace の ASIL D 認証、Apex.Ida）: https://jobs.lever.co/apex-ai/db890dbb-8232-4b60-9c89-f6cee5fb4a4a
- Green Hills: Platforms for Automotive（INTEGRITY の ASIL D、EAL 6+、Multivisor）: https://www.ghs.com/products/auto_autosar.html
- Green Hills: Platform for Safe and Secure Automated Driving Systems（dual-channel の plausibility analysis）: https://www.ghs.com/products/auto_adas.html
- NXP と Green Hills の自動運転向け提携（CAN Newsletter）: https://can-newsletter.org/catalogue/do/printArticle/id/200129_nxp-teams-with-green-hills-software-on-platform-for-autonomous-driving_nxp-green-hills-software/200129_nxp-teams-with-green-hills-software-on-platform-for-autonomous-driving_nxp-green-hills-software.pdf
- SYSGO PikeOS（RTOS & Hypervisor、認証一覧）: https://www.sysgo.com/rtos-hypervisor
- SYSGO: 防衛由来 architecture の自動車への適用（FFI、mixed criticality）: https://www.sysgo.com/blog/article/secure-safe-certified-how-defense-inspired-architectures-are-powering-the-future-of-vehicles
- Wind River VxWorks for Automotive（ASIL D 認証、TÜV SÜD）: https://www.windriver.com/resource/vxworks-for-automotive-solution-brief
- Wind River VxWorks Cert Edition（certification evidence）: http://www.windriver.com/resource/vxworks-cert-edition-product-overview
- Wind River Helix Virtualization Platform: https://www.windriver.com/resource/helix-platform-product-overview
- Elektrobit EB corbos Linux for Safety Applications（ASIL B / SIL 2、TÜV Nord の technical assessment、hypervisor 監視）: https://www.elektrobit.com/products/ecu/eb-corbos/linux-for-safety-applications/
- Mobileye Drive への EB corbos Linux 統合（Automotive World）: https://www.automotiveworld.com/news/mobileye-drive-adopts-elektrobit-safety-compliant-linux-os/
- Android Automotive（Wikipedia、採用 OEM と GAS の説明）: https://en.wikipedia.org/wiki/Android_Automotive_OS
- Autoware（DEPLOY registry、Autoware Foundation と Tier IV、商用展開）: https://registry.deploy.report/brains/autoware
- Autoware と Apollo の比較（Cyber RT、ROS 2 / DDS）: https://www.cosmiclearn.com/autoveh/autoware-apollo.php

規格・規制:

- 自動運転の compliance 整理（ISO 26262、ISO 21448、UL 4600、ISO/TS 5083、ISO/SAE 21434 の版と適用範囲）: https://www.hazloclabs.com/services/autonomous-vehicles/
- ISO 26262 と SOTIF の解説: https://www.cosmiclearn.com/autoveh/safety-standards-iso26262.php
- UN R155 / R156 と ISO/SAE 21434 の解説（発効日、EU 適用日、RXSWIN、SUMS）: https://softwsp.com/guides/automotive-cybersecurity-un-r155-r156/
- UL Solutions の自動運転安全 training / advisory（UL 4600、ISO 26262、ISO 21448、ISO/SAE 21434 の位置づけ）: https://ul-com.agentsite.network/content/services/autonomous-vehicle-safety-training-and-advisory/index.html

業界動向:

- Domain vs Zonal Architecture（Automotive IQ、IoT Analytics と McKinsey の数値）: https://www.automotive-iq.com/autonomous-drive/articles/domain-vs-zonal-architecture-the-engineering-shift-behind-software-defined-vehicles
- Software-defined vehicles: the OEM guide（Cubic、OEM 各社の動向）: https://www.cubic3.com/blog/software-defined-vehicles/
- SOAFEE（Arm の success library、150 超の partner、cloud-native）: https://www.arm.com/company/success-library/made-possible/soafee
- Arm Learning Paths: SDV と SOAFEE、Open AD Kit: https://learn.arm.com/learning-paths/automotive/openadkit1_container/1_sdv_soafee/
- VDA / Eclipse Foundation 共同発表（MoU の 32 社への拡大、Eclipse S-CORE）: https://www.vda.de/en/press/press-releases/2026/260107_Automotive-innovation-through-open-collaboration--Momentum-builds-around-open-source-software-as-a-key-driver-of-efficiency-and-success
- ETAS: Eclipse S-CORE 0.5 Alpha 発表（ASPICE、ISO 26262、ISO/SAE 21434 への適合方針）: https://www.etas.com/ww/en/about-etas/press-room/press-releases/eclipse-s-core-0-5-alpha-a-major-step-forward-for-open-and-safe-automotive-software-platforms/

Sigil 側の参照（リポジトリ内）:

- ADR 0005（長期目標と段階計画、P6 の位置づけ）: `docs/decisions/0005-long-term-goals.md`
- ADR 0027（AI が書く前提の安全設計原則）: `docs/decisions/0027-ai-era-security-design-principles.md`
- ADR 0004（効果）: `docs/decisions/0004-effects.md`

---

## 付記: 本レポートの限界

- 認証の有無はベンダー発表と認証機関への言及に基づく。個別の証明書の適用範囲（対象 version、構成、前提条件）は各社の safety manual 等の非公開資料に依存する部分があり、本レポートでは確認できていない。
- 市場規模や搭載台数などの数値は、公表主体（ベンダー、調査会社）の自己申告を含む。意思決定の根拠にする場合は原典の確認が要る。
- 規格の版・改訂状況（特に ISO 26262 第 3 版、UN 規則の改正）は 2026-11 時点の公開情報に基づく。
