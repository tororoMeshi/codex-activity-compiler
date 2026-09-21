# Codex Activity Compiler

Codexが出力する大量のOpenTelemetryデータを、
人間が理解できる Session / Turn / ToolCall に変換して保存する
軽量なローカル観測ツール。

Raw Traceを保存することは目的としない。

詳細: [Project Design Brief](docs/PROJECT.md)

## ローカル実行

Rustの安定版toolchainが必要です。次で、SQLiteファイル `codex-activity.sqlite` と
OTLP/HTTP traces endpointをローカルに起動します。

```bash
cargo run
```

受信先は `http://127.0.0.1:4318/v1/traces` です。OTLP exporterはprotobuf形式の
`POST /v1/traces` を送信してください。

保存先と待受アドレスは環境変数で変更できます。

```bash
CODEX_ACTIVITY_DB=./activity.sqlite \
CODEX_ACTIVITY_LISTEN=127.0.0.1:4318 \
cargo run
```

アプリケーションは受信時にCodex Telemetryを `Session`、`Turn`、`ToolCall`、
`UnknownTelemetry` に正規化してSQLiteへ保存し、OTLPのRaw Span/Event、prompt本文、
tool arguments、tool output、trace/parent ID、resource属性は保存しません。

開発時の検証は次で実行できます。

```bash
cargo fmt --check
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
git diff --check
```
