# Codex OTLP から開発活動へのマッピング

## 1. 結論と前提

この文書は、`docs/PROJECT.md` を Source of Truth とし、Codex 0.155.1
から採取した `~/codex-spans.jsonl`（2,000 Span）だけを根拠にする。
保存するのは Raw Span ではなく、そこから取り出して正規化した
`Session`、`Turn`、`ToolCall` である。ここで「残す」は当該の人間向け
レコードのフィールドに変換して保存する意味であり、Span/Event 本体を
保存する意味ではない。

標本には 44 個の `metadata.opentelemetry.events` があり、次の意味イベント
を確認した。したがって Span 名だけではなく、必ず Event の
`attributes["event.name"]` を先に判定する。

| 実測 Event | 件数 | 親 Span | 分類 | 用途 |
| --- | ---: | --- | --- | --- |
| `codex.tool_result` | 33 | `dispatch_tool_call_with_terminal_outcome` | 残す | ToolCall の正本 |
| `codex.api_request` | 5 | `endpoint_session.execute_with` | 捨てる | API リクエスト内部計測。活動モデルのフィールドを増やさない |
| `codex.startup_phase` | 3 | `startup_prewarm.resolve`、`thread/start` | 捨てる | 起動処理の内部計測。Session の境界・属性の正本ではない |
| `codex.turn_ttft` | 1 | `try_run_sampling_request` | 残す | Turn の TTFT |
| `codex.user_prompt` | 1 | `op.dispatch.turn_input` | 集計だけ | 本文を除く prompt 指標 |
| `codex.websocket_request` | 1 | `responses_websocket.stream_request` | 捨てる | WebSocket 転送の内部計測。活動モデルのフィールドを増やさない |

## 2. 実測した主要 Span と分類

| Span / Event | 標本で確認した意味情報 | 分類 | 保存しない理由または使い道 |
| --- | --- | --- | --- |
| `session_task.turn` | `turn.id`、`thread.id`、model、reasoning effort、token usage、開始・終了・duration | 残す | 完了した Turn の正本 |
| `run_sampling_request` | `turn_id`、`cwd`、model | 残す | Turn に紐付く `cwd` の正本。model は `session_task.turn` がある場合そちらを優先 |
| `try_run_sampling_request` 内 `codex.turn_ttft` | Span の `turn_id` と Event の `duration_ms` | 残す | TTFT の正本。Span 自体は結合補助だけ |
| `op.dispatch.turn_input` 内 `codex.user_prompt` | `conversation.id`、`prompt_length`、入力件数 | 残す（指標のみ） | 確定的に結べる Turn の prompt 長にだけ変換し、本文は保存しない |
| `turn/start` | `turn.id`、trace/parent 関係 | 集計だけ | prompt と canonical Turn を結ぶ短命の補助情報 |
| `dispatch_tool_call_with_terminal_outcome` 内 `codex.tool_result` | call ID、名称、origin、成功、入出力サイズ、tool duration | 残す | ToolCall の正本。親 Span の duration は使わない |
| `mcp.tools.call` | `tool.call_id`、`conversation.id`、`turn.id`、MCP error 属性、Span duration | 集計だけ | `call_id` による Turn 結合と診断補助。ToolCall を二重作成しない |
| `handle_responses` | model、reasoning、usage 等の重複した Turn 情報 | 捨てる | `session_task.turn` と同じ Turn 情報の別計測で、二重計上を避ける |
| `append_items` | 内部 item 追加の属性。標本では人間向け ID・usage・tool 結果 Event を持たない | 捨てる | 高頻度のキュー操作であり、活動モデルに追加の意味を与えない |
| `persist_rollout_items` | 永続化対象数などの内部 rollout 属性。意味 Event はない | 捨てる | 保存機構の計測であってユーザーの開発活動ではない |
| `realtime_conversation.running_state` | running/idle 等の内部状態属性。意味 Event はない | 捨てる | 瞬間状態であり、Session/Turn/ToolCall の正本でも境界でもない |
| `auth` | 認証処理の内部属性。意味 Event はない | 捨てる | 認証状態は活動の内容・成否・時間を追加で説明しない。認証情報も保存しない |
| `responses_websocket.stream_request` | WebSocket 転送の内部状態 | 捨てる（上表の Event だけ例外） | Session の開始・終了を接続寿命から推定しない |
| 上記以外の Span / Event | この標本では人間向けモデルへの確定的な対応を確認できない | 未知 | §9 の最小統計だけを残す |

`append_items`、`persist_rollout_items`、`realtime_conversation.running_state`、
`auth` は、単に名前から除外したものではない。標本の `input`/`attributes` と
`metadata.opentelemetry.events` を確認した結果、いずれにも conversation/turn
を人間向けに説明する token、tool terminal outcome、TTFT、prompt 指標はなく、
内部の投入・永続化・状態遷移・認証を示すだけだった。

## 3. 関連付けと正規化規則

### 3.1 キー

* Session の内部キーは `conversation.id` である。保存時の一意キーもこれである。
* Turn の内部キーは `turn.id` である。保存時は `(conversation.id, turn.id)` を
  一意キーとする。
* ToolCall の内部キーは `(conversation.id, call_id)` とする。
* `cwd` は `name == "run_sampling_request"` の `input["cwd"]` を、同じ
  Span の `input["turn_id"]` の Turn に結ぶ。標本ではここに
  `turn_id`、`cwd`、model が同時に存在した。

`name == "session_task.turn"` の `input["thread.id"]` は、標本で
`codex.user_prompt` Event の `attributes["conversation.id"]` と同じ値だった。
ただし `thread.id` 単独を会話 ID と仮定しない。`op.dispatch.turn_input` の
`codex.user_prompt` Event と、
trace/parent 関係で同じ Turn に到達できる場合にだけ Session を確定する。
この規則で trace 内の `turn/start` と `session_task.turn` の各
`input["turn.id"]`、および prompt event を結ぶ。結合に使った trace/parent ID
自体は保存しない。

`name == "mcp.tools.call"` の `input["conversation.id"]` と
`input["turn.id"]` は明示的な結合キーである。MCP ToolCall は、同じ `call_id`
の `codex.tool_result` にこの Turn を付与する。

### 3.2 Session フィールド

| 保存フィールド | 取得元と規則 |
| --- | --- |
| `id` | 確定した `conversation.id` |

Session の開始・終了時刻、Turn数、ToolCall数、token合計は、保存済みのTurnと
ToolCallから導出するため保存しない。token合計は canonical Turn の
`input_tokens + output_tokens` とし、`total_tokens` を Session 合計に再集計
しない。欠損値を他の token 値から補完しない。ToolCall数には `turn_id == null`
の結果も含める。

### 3.3 Turn フィールド

| 保存フィールド | 取得元と規則 |
| --- | --- |
| `id`、`session_id` | §3.1 の `input["turn.id"]`、確定した `conversation.id` |
| `model` | `name == "session_task.turn"` の `input["model"]`。欠ける場合だけ同一 `turn_id` の `run_sampling_request` の `input["model"]` |
| `reasoning_effort` | `name == "session_task.turn"` の `input["codex.turn.reasoning_effort"]`（§7） |
| `started_at` / `ended_at` | `name == "session_task.turn"` の top-level `start_time` / `end_time`。`duration_ms` は保存せず、`ended_at - started_at` から導出する |
| `ttft_ms` | §8 の `codex.turn_ttft` |
| `input_tokens` 等 | §6 の usage 属性 |
| `cwd` | 同じ `turn_id` の `run_sampling_request` の `input["cwd"]`。Session に単一値としては保存しない |
| `prompt_length` | trace/parent 関係で canonical Turn に確定的に到達できる `codex.user_prompt` Event の `attributes["prompt_length"]`。本文・配列内容は保存しない |

### 3.4 ToolCall フィールド

`metadata["opentelemetry.events"][]` 内で
`attributes["event.name"] == "codex.tool_result"` の Event attributes を使う。
Span の `input` / `output` は使わない。

| 保存フィールド | Event attribute |
| --- | --- |
| `id` | `call_id` |
| `session_id` | `conversation.id` |
| `name` | `tool_name` |
| `kind` | §5 の builtin / MCP 判定 |
| `success` | `success` |
| `input_bytes` | `arguments_length` |
| `output_bytes` | `output_length` |
| `output_line_count` | `output_line_count` |
| `output_truncated` | `output_truncated` |
| `duration_ms` | `duration_ms`（§4） |
| `completed_at` | Event の timestamp。ToolCall 開始時刻は、この標本だけでは確定しないため導出して保存しない |

`codex.tool_result` には `conversation.id` はあるが、標本では builtin Tool の
`turn.id` はなかった。MCP は前述の `mcp.tools.call` との `call_id` 結合で
Turn を得られる。builtin は、trace/parent 関係により canonical Turn へ
**確定的に**到達できる場合だけ結ぶ。時刻が近い、同一 `cwd`、同一モデルだけを
根拠に Turn を推測してはならない。この標本には、12 件の builtin result に
その確定的な `turn.id` を示す属性はなかった。この未解決時の保存形は §10 の
実装前決定事項である。

## 4. ToolCall の重複排除、成否、duration

標本では `codex.tool_result` 33 件のうち 21 件が `mcp.tools.call` と同じ
`call_id` を持ち、12 件は builtin だった。MCP Span の duration と Event の
`duration_ms` は数 ms 異なった。よって次を正本規則とする。

1. `codex.tool_result` を一件の ToolCall とする。キーは
   `(conversation.id, call_id)`。
2. 同キーの `mcp.tools.call` は補助情報として結合するだけで、ToolCall を
   追加作成しない。
3. 同じ `codex.tool_result` が複数来た場合は同キーを一件に畳み、Event timestamp
   が最も新しいものを terminal outcome とする。重複件数は Session/Turn 件数に
   加えない。
4. `duration_ms` は常に `codex.tool_result.duration_ms` を採用する。親の
   `dispatch_tool_call_with_terminal_outcome.duration`、`mcp.tools.call.duration`
   を代替値にも合算値にも使わない。
5. 成否は `codex.tool_result.success` が唯一の正本である。`false` は失敗として
   ToolCall を保存する。`mcp.tools.call` の `error.type` と
   `codex.mcp.error.code` は失敗診断には使えるが、event の `success` を上書き
   しない。標本には `success=false` と MCP error 属性を持つ失敗例が 1 件あった。

## 5. builtin tool と MCP tool

`codex.tool_result` の `mcp_tool` と `tool_origin` を併用する。

* `mcp_tool == true` かつ `tool_origin == "mcp"` は MCP。
* `mcp_tool == false` かつ `tool_origin == "builtin"` は builtin。
* 欠損または矛盾は `unknown` とし、`tool_namespace` の文字列から推測しない。

標本ではこの判定が、MCP の `mcp.tools.call` との `call_id` 結合結果と整合した。
`tool_namespace` は表示補助になるが、最小モデルの分類キーにはしない。

## 6. Token usage

token usage は `name == "session_task.turn"` の `input` にある次の属性を
そのまま Turn に保存する。

* `codex.turn.token_usage.input_tokens`
* `codex.turn.token_usage.cached_input_tokens`
* `codex.turn.token_usage.output_tokens`
* `codex.turn.token_usage.reasoning_output_tokens`
* `codex.turn.token_usage.total_tokens`

Session の token 合計が必要な場合は、重複した `handle_responses` ではなく
canonical Turn の `total_tokens` を合計する。`total_tokens` が欠損する Turn を
`input_tokens + output_tokens` で補完しない。`input_tokens`、
`cached_input_tokens`、`output_tokens`、`reasoning_output_tokens` はそれぞれ独立した
観測値として保存し、`total_tokens` との包含関係・算術関係を導出規則にしない。

## 7. Reasoning effort

Turn の reasoning effort は `name == "session_task.turn"` の
`input["codex.turn.reasoning_effort"]` を採る。
標本で `handle_responses` にも似た情報があったが、二重計上・競合を避けるため
補完元にしない。属性がない場合は `null` のままとし、モデル名や token 数から
推測しない。

## 8. TTFT

`metadata["opentelemetry.events"][]` の
`attributes["event.name"] == "codex.turn_ttft"` にある
`attributes["duration_ms"]` を `Turn.ttft_ms` に保存する。event を含む
`try_run_sampling_request` Span の `input["turn_id"]` で Turn を特定する。
標本にはこの Event が 1 件あり、`run_sampling_request` の span duration や
WebSocket/API span の時間を TTFT の代用にしない。

## 9. 未知の Span / Event

分類表にない名前は、Raw を保持せず未知統計に畳む。各未知項目は次の五つだけを
保存する。

| フィールド | 規則 |
| --- | --- |
| `name` | Span 名、または Event の `event.name`。`kind` と組で識別する |
| `kind` | 観測種別の `Span` または `Event` |
| `count` | 観測回数 |
| `first_seen` | 最初の Span/Event timestamp |
| `last_seen` | 最後の Span/Event timestamp |

attributes、resource、trace ID、parent ID、input、output は未知統計にも保存しない。
既知の高頻度内部 Span を、将来の名前変更だけを理由に人間向けレコードへ昇格させない。

## 10. 決定済みの制約

* builtin の `codex.tool_result` は `ToolCall.turn_id` を nullable とする。
  `mcp.tools.call` または trace/parent 関係で canonical Turn へ確定的に到達できる
  場合だけ値を入れる。時刻近接、同一 `cwd`、同一モデルは結合根拠にしない。
* 一つの Session で `cwd` が変わっても Session の値は上書きも履歴化もしない。
  `run_sampling_request` の `input["turn_id"]` で確定的に結べる `Turn.cwd` としてだけ保持する。
  `cwd` を持たない Turn は `null` とし、履歴用テーブルは追加しない。
* `codex.user_prompt` の `attributes["prompt_length"]` は、canonical Turn へ確定的に
  結べる場合だけ `Turn.prompt_length` に保存する。本文・配列内容は保存しない。

これら以外を推測仕様で補わない。

## 11. Raw Span を保存しない損失と、意図して捨てる情報

Raw を残さないため、完全な trace/parent グラフ、内部 retry・キュー・永続化・
WebSocket の時系列、各 SDK/ネットワーク Span の詳細、resource 属性、認証・
接続状態、MCP の低レベル診断を後から再解析できない。また、未知属性を追加して
調べ直すこともできない。

これは意図的な境界である。特に User Prompt 本文、Tool arguments、Tool Output
本文、API payload、認証関連値、内部 item 内容は保存しない。保存するサイズ、件数、
成否、duration、token usage、reasoning effort、TTFT は、人間が開発活動を理解する
ために必要な最小限である。
