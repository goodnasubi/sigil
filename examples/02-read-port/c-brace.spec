モジュール examples.read_port
  出力先: サーバー
  信頼基盤: いいえ

型 Port
  基にする型: Int
  条件: 1 <= self && self <= 65535

列挙型 ConfigError
  バリアント: Io（IoError）、NotANumber（String）、OutOfRange（Int）

関数 read_port
  意図: 設定ファイルの内容を整数として読み、ポート番号として返す
  引数: path を受け取る（Path）
  戻り値: Result<Port, ConfigError>
  事前条件: !path.is_empty()
  事後条件: なし
  効果: ファイルを読む
