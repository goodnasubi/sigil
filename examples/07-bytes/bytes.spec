モジュール examples.bytes
  出力先: サーバー
  信頼基盤: いいえ

関数 build_packet（非公開）
  意図: 種別と長さを前に付けたバイナリメッセージを組み立てる
  引数: tag を受け取る（Int）、first を受け取る（Int）、second を受け取る（Int）
  戻り値: Bytes
  事前条件: 0 <= tag && tag <= 255 && 0 <= first && first <= 255 && 0 <= second && second <= 255
  事後条件: result.len() == 7
  効果: なし

関数 packet_tag（非公開）
  意図: メッセージの種別バイトを返す
  引数: packet を読む（Bytes）
  戻り値: Int
  事前条件: なし
  事後条件: なし
  効果: なし

関数 declared_len（非公開）
  意図: メッセージに書かれた本体の長さを返す
  引数: packet を読む（Bytes）
  戻り値: Int
  事前条件: なし
  事後条件: なし
  効果: なし

関数 checksum（非公開）
  意図: 全バイトを XOR と回転で混ぜたチェックサムを返す
  引数: packet を読む（Bytes）
  戻り値: Int
  事前条件: なし
  事後条件: なし
  効果: なし

関数 scramble（非公開）
  意図: 下位 2 バイトを取り出し、回転と XOR で混ぜる
  引数: x を受け取る（Int）
  戻り値: Int
  事前条件: なし
  事後条件: なし
  効果: なし

関数 sample_len（非公開）
  意図: 組み立てたメッセージ全体の長さを返す
  引数: tag を受け取る（Int）、first を受け取る（Int）、second を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= tag && tag <= 255 && 0 <= first && first <= 255 && 0 <= second && second <= 255
  事後条件: result == 7
  効果: なし

関数 sample_tag（非公開）
  意図: 組み立てたメッセージの種別バイトを返す
  引数: tag を受け取る（Int）、first を受け取る（Int）、second を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= tag && tag <= 255 && 0 <= first && first <= 255 && 0 <= second && second <= 255
  事後条件: result == tag
  効果: なし

関数 sample_declared_len（非公開）
  意図: 組み立てたメッセージに書かれた本体の長さを返す
  引数: tag を受け取る（Int）、first を受け取る（Int）、second を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= tag && tag <= 255 && 0 <= first && first <= 255 && 0 <= second && second <= 255
  事後条件: result == 2
  効果: なし

関数 sample_checksum（非公開）
  意図: 組み立てたメッセージのチェックサムを返す
  引数: tag を受け取る（Int）、first を受け取る（Int）、second を受け取る（Int）
  戻り値: Int
  事前条件: 0 <= tag && tag <= 255 && 0 <= first && first <= 255 && 0 <= second && second <= 255
  事後条件: なし
  効果: なし
