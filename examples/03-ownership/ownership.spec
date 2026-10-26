モジュール examples.ownership
  出力先: ブラウザ, サーバー
  信頼基盤: いいえ

構造体 Account（非公開）
  フィールド: owner（String）、balance（Int）

関数 balance_of（非公開）
  意図: 口座の残高を返す
  引数: account を読む（Account）
  戻り値: Int
  事前条件: なし
  事後条件: result == account.balance
  効果: なし

関数 deposit（非公開）
  意図: 口座に入金する
  引数: account を書き換える（Account）、amount を受け取る（Int）
  戻り値: なし
  事前条件: amount > 0
  事後条件: account.balance == old(account.balance) + amount
  効果: なし

関数 close（非公開）
  意図: 口座を閉じ、払い戻す金額を返す
  引数: account を受け取る（Account）
  戻り値: Int
  事前条件: account.balance >= 0
  事後条件: result == account.balance
  効果: なし

関数 richest（非公開）
  意図: 残高が最大の口座を返す。口座がなければ none を返す
  引数: accounts を読む（List<Account>）
  戻り値: Option<&Account>
  事前条件: なし
  事後条件: なし
  効果: なし

関数 main（非公開）
  意図: 所有権と借用の使い方を示す
  引数: なし
  戻り値: なし
  事前条件: なし
  事後条件: なし
  効果: コンソールに出力する
