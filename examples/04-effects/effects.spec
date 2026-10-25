モジュール examples.effect_check
  出力先: サーバー
  信頼基盤: いいえ

関数 add_tax
  意図: 税込み価格を返す
  引数: price を受け取る（Int）、rate_percent を受け取る（Int）
  戻り値: Int
  事前条件: price >= 0 && rate_percent >= 0
  事後条件: result >= price
  効果: なし

関数 load_prices
  意図: 価格の一覧をファイルから読む
  引数: path を受け取る（Path）
  戻り値: Result<List<Int>, IoError>
  事前条件: !empty(path)
  事後条件: なし
  効果: コンソールに出力する、ファイルを読む

関数 apply_all
  意図: 一覧の各要素に f を適用した一覧を返す
  引数: xs を読む（List<Int>）、f を受け取る（fn(Int) -> Int）
  戻り値: List<Int>
  事前条件: なし
  事後条件: len(result) == len(xs)
  効果: f の効果

関数 with_tax
  意図: すべての価格に 10% の税を加える
  引数: prices を読む（List<Int>）
  戻り値: List<Int>
  事前条件: なし
  事後条件: len(result) == len(prices)
  効果: なし

関数 count_up
  意図: 共有のカウンタを 1 増やす
  引数: counter を読む（Shared<Cell<Int>>）
  戻り値: なし
  事前条件: なし
  事後条件: なし
  効果: 共有所有の型を通して値を書き換える
