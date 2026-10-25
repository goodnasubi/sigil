モジュール examples.control
  出力先: ブラウザ, サーバー
  信頼基盤: いいえ

関数 sum_range
  意図: from 以上 to 以下の整数の合計を返す
  引数: from を受け取る（Int）、to を受け取る（Int）
  戻り値: Int
  事前条件: from <= to
  事後条件: なし
  効果: なし

関数 clamp
  意図: x を low 以上 high 以下に収める
  引数: x を受け取る（Int）、low を受け取る（Int）、high を受け取る（Int）
  戻り値: Int
  事前条件: low <= high
  事後条件: low <= result && result <= high
  効果: なし

関数 sign
  意図: x が負なら -1、それ以外なら 1 を返す
  引数: x を受け取る（Int）
  戻り値: Int
  事前条件: なし
  事後条件: result == -1 || result == 1
  効果: なし

関数 add_to
  意図: x に n を加えた値を返す
  引数: n を受け取る（Int）、x を受け取る（Int）
  戻り値: Int
  事前条件: なし
  事後条件: result == x + n
  効果: なし
