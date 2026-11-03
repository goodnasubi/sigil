モジュール examples.traits
  出力先: サーバー
  信頼基盤: いいえ

トレイト Shape（非公開）
  関数 area
    意図: 面積を返す
    引数: self を読む（Self）
    戻り値: Int
    事前条件: なし
    事後条件: result >= 0
    効果: なし
  関数 label
    意図: 図形の名前を返す
    引数: self を読む（Self）
    戻り値: String
    事前条件: なし
    事後条件: なし
    効果: なし

構造体 Square（非公開）
  フィールド: side（Int）

構造体 Rectangle（非公開）
  フィールド: w（Int）、h（Int）

メソッド Square.area（非公開）
  トレイト: Shape
  意図: 面積を返す
  引数: self を読む（Square）
  戻り値: Int
  事前条件: なし
  事後条件: result >= 0
  効果: なし

メソッド Square.label（非公開）
  トレイト: Shape
  意図: 図形の名前を返す
  引数: self を読む（Square）
  戻り値: String
  事前条件: なし
  事後条件: なし
  効果: なし

メソッド Rectangle.area（非公開）
  トレイト: Shape
  意図: 面積を返す
  引数: self を読む（Rectangle）
  戻り値: Int
  事前条件: なし
  事後条件: result >= 0
  効果: なし

メソッド Rectangle.label（非公開）
  トレイト: Shape
  意図: 図形の名前を返す
  引数: self を読む（Rectangle）
  戻り値: String
  事前条件: なし
  事後条件: なし
  効果: なし

関数 doubled_area<T: Shape>（非公開）
  意図: 面積の 2 倍を返す
  引数: s を受け取る（T）
  戻り値: Int
  事前条件: なし
  事後条件: result >= 0
  効果: なし

関数 square_area（非公開）
  意図: 正方形の面積を返す
  引数: side を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= side
  事後条件: result >= 0
  効果: なし

関数 rectangle_area（非公開）
  意図: 長方形の面積を返す
  引数: w を受け取る（Int）、h を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= w && 0 <= h
  事後条件: result >= 0
  効果: なし

関数 square_doubled_area（非公開）
  意図: 正方形の面積の 2 倍を、総称の制約越しに返す
  引数: side を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= side
  事後条件: result >= 0
  効果: なし

関数 rectangle_label（非公開）
  意図: 長方形の名前を返す
  引数: w を受け取る（Int）、h を受け取る（Int）
  戻り値: String
  事前条件: なし
  事後条件: なし
  効果: なし
