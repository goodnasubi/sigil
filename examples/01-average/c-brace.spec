モジュール examples.average
  出力先: ブラウザ, サーバー
  信頼基盤: いいえ

関数 average
  意図: 数値列の算術平均を返す
  引数: xs を受け取る（List<Float>）
  戻り値: Float
  事前条件: len(xs) > 0
  事後条件: min(xs) <= result && result <= max(xs)
  効果: なし
