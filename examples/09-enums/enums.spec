モジュール examples.enums
  出力先: サーバー
  信頼基盤: いいえ

列挙型 Order（非公開）
  バリアント: Received（Int）、Paid（Int, Int）、Shipped（Int, Int）、Cancelled（Int）

関数 order_id_of（非公開）
  意図: 注文番号を返す
  引数: o を受け取る（Order）
  戻り値: Int
  事前条件: なし
  事後条件: なし
  効果: なし

関数 state_code（非公開）
  意図: 状態の番号を返す（受付 0・支払済 1・発送済 2・取消 3）
  引数: o を受け取る（Order）
  戻り値: Int
  事前条件: なし
  事後条件: 0 <= result && result <= 3
  効果: なし

関数 paid_amount（非公開）
  意図: 支払済なら支払額、それ以外は 0 を返す
  引数: o を受け取る（Order）
  戻り値: Int
  事前条件: なし
  事後条件: なし
  効果: なし

関数 is_finished（非公開）
  意図: 発送済か取消なら true を返す
  引数: o を受け取る（Order）
  戻り値: Bool
  事前条件: なし
  事後条件: なし
  効果: なし

関数 pay（非公開）
  意図: 受付の注文を支払済にする。それ以外は同じ状態を返す
  引数: o を受け取る（Order）、amount を受け取る（Int）
  戻り値: Order
  事前条件: 0 <= amount
  事後条件: なし
  効果: なし

関数 ship（非公開）
  意図: 支払済の注文を発送済にする。それ以外は同じ状態を返す
  引数: o を受け取る（Order）、tracking を受け取る（Int）
  戻り値: Order
  事前条件: なし
  事後条件: なし
  効果: なし

関数 cancel（非公開）
  意図: 発送前なら取り消す。発送済と取消は同じ状態を返す
  引数: o を受け取る（Order）
  戻り値: Order
  事前条件: なし
  事後条件: なし
  効果: なし

関数 received_id（非公開）
  意図: 受付の注文の番号を返す
  引数: id を受け取る（Int）
  戻り値: Int
  事前条件: なし
  事後条件: result == id
  効果: なし

関数 paid_code_and_amount（非公開）
  意図: 支払った注文の、状態番号と支払額の合計を返す
  引数: id を受け取る（Int）、amount を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= amount
  事後条件: なし
  効果: なし

関数 shipped_code（非公開）
  意図: 支払って発送した注文の状態番号を返す
  引数: id を受け取る（Int）、amount を受け取る（Int）、tracking を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= amount
  事後条件: result == 2
  効果: なし

関数 shipped_cancel_code（非公開）
  意図: 発送した注文は取り消せないこと（状態番号が発送済のまま）を返す
  引数: id を受け取る（Int）、amount を受け取る（Int）、tracking を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= amount
  事後条件: result == 2
  効果: なし

関数 paid_cancel_finished（非公開）
  意図: 支払済を取り消すと、終わった状態になることを返す
  引数: id を受け取る（Int）、amount を受け取る（Int）
  戻り値: Bool
  事前条件: 0 <= amount
  事後条件: result == true
  効果: なし
