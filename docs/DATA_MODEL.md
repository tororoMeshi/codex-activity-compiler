# MVP 論理データモデル

## 目的と境界

このモデルは、Codex の Telemetry をそのまま保存するものではない。Telemetry から
**確定的に**読み取れる情報だけを、人間が理解する `Session`、`Turn`、`ToolCall` に
変換して保存する。Raw Span、trace/parent ID、attributes、resource、入力・出力本文は
保存しない。

結合に利用してよいのは、`conversation.id`、`turn.id`、`call_id` と、Telemetry Mapping
で定義した trace/parent 関係だけである。時刻近接、同一 `cwd`、同一モデルなどからの
関連推定はしない。Telemetry に値がなければ `null` のままとする。

型の `timestamp` は観測時刻、`integer` は非負整数、`text` はTelemetryから得た文字列、
`boolean` は真偽値である。ここでは論理モデルのみを定義し、DDL・物理型・索引は定義しない。

## 関係と一意性

* `Session.id` は `conversation.id` であり、一意である。
* `Turn` の一意性は `(session_id, id)`、`ToolCall` の一意性は `(session_id, id)` である。
* Turn は一つのSessionに属する。ToolCallも必ず一つのSessionに属するが、Turnへの所属は
  確定できないことがある。
* `UnknownTelemetry` の一意性は `(kind, name)` である。Session、Turn、ToolCallには属さない。

## Session

Session は一つの `conversation.id` による開発活動のまとまりである。開始・終了時刻、Turn数、
ToolCall数、token合計は下位レコードから導出できるため保存しない。token合計が必要な場合は
canonical Turn の `total_tokens` を合計する。`total_tokens` が欠損する Turn を他の token 値から
補完しない。`cwd` もSessionの属性にはしない。

| フィールド | 型 | 必須 / nullable | 情報源となるTelemetry | 保存する理由 |
| --- | --- | --- | --- | --- |
| `id` | text | 必須 | 確定した `conversation.id` | TurnとToolCallを、人間が理解する一つの開発セッションへまとめる識別子。 |

## Turn

Turn は `session_task.turn` を正本とする、完了した一回のCodex応答である。`duration_ms` は
`ended_at - started_at` で導出できるため保存しない。各値は正本または定めた確定的な補助Telemetry
からのみ取得する。token の各フィールドは独立した観測値として保存し、相互の包含関係・算術関係を
導出規則にしない。

| フィールド | 型 | 必須 / nullable | 情報源となるTelemetry | 保存する理由 |
| --- | --- | --- | --- | --- |
| `id` | text | 必須 | `name == "session_task.turn"` の `input["turn.id"]` | Session内のTurnを識別する。 |
| `session_id` | text | 必須 | trace/parent関係で結べる `codex.user_prompt` Event の `attributes["conversation.id"]` | 親Sessionへの所属を表す。 |
| `model` | text | nullable | `session_task.turn` の `input["model"]`。欠ける場合だけ同じ `turn_id` の `run_sampling_request` の `input["model"]` | どのモデルで応答した開発活動かを示す。推測では補わない。 |
| `reasoning_effort` | text | nullable | `session_task.turn` の `input["codex.turn.reasoning_effort"]` | 応答時に指定された推論努力を示す。 |
| `started_at` | timestamp | 必須 | `session_task.turn` の top-level `start_time` | Turnの時系列と所要時間の導出に必要。 |
| `ended_at` | timestamp | 必須 | `session_task.turn` の top-level `end_time` | Turnの時系列と所要時間の導出に必要。 |
| `ttft_ms` | integer | nullable | Event の `attributes["event.name"] == "codex.turn_ttft"` と `attributes["duration_ms"]`。親 `try_run_sampling_request` の `input["turn_id"]` で結ぶ | 最初のトークンまでの時間を、代替推定せずに示す。 |
| `input_tokens` | integer | nullable | `session_task.turn` の `input["codex.turn.token_usage.input_tokens"]` | Turnの入力規模を示す。 |
| `cached_input_tokens` | integer | nullable | `session_task.turn` の `input["codex.turn.token_usage.cached_input_tokens"]` | 入力tokenのキャッシュ利用量を区別する。 |
| `output_tokens` | integer | nullable | `session_task.turn` の `input["codex.turn.token_usage.output_tokens"]` | Turnの出力規模を示す。 |
| `reasoning_output_tokens` | integer | nullable | `session_task.turn` の `input["codex.turn.token_usage.reasoning_output_tokens"]` | 推論出力token量を区別する。 |
| `total_tokens` | integer | nullable | `session_task.turn` の `input["codex.turn.token_usage.total_tokens"]` | Telemetryが提示する合計値をそのまま示す。入出力tokenの和で再計算しない。 |
| `cwd` | text | nullable | 同じ `turn_id` の `run_sampling_request` の `input["cwd"]` | Session中の作業ディレクトリ変化を、上書き・履歴推定なしで各Turnの観測値として表す。 |
| `prompt_length` | integer | nullable | trace/parent関係でcanonical Turnに確定的に結べる `codex.user_prompt` Event の `attributes["prompt_length"]` | 本文を保存せず、そのTurnの入力規模を人間が把握できるようにする。 |

## ToolCall

ToolCall は `metadata["opentelemetry.events"][]` 内で
`attributes["event.name"] == "codex.tool_result"` である Event を正本とする、一回の
終端結果である。同じ `(conversation.id, call_id)` の `mcp.tools.call` はTurn結合の補助だけに
使い、別レコードを作らない。同じ結果が複数来た場合は、最も新しいEvent timestampを終端結果とする。

| フィールド | 型 | 必須 / nullable | 情報源となるTelemetry | 保存する理由 |
| --- | --- | --- | --- | --- |
| `id` | text | 必須 | 正本 Event の `attributes["call_id"]` | Session内のToolCallを識別し、重複を排除する。 |
| `session_id` | text | 必須 | 正本 Event の `attributes["conversation.id"]` | 親Sessionへの所属を確定的に表す。 |
| `turn_id` | text | nullable | 同じ `call_id` の `mcp.tools.call` の `input["turn.id"]`、またはtrace/parent関係でcanonical Turnへ確定的に到達した値 | MCP等で確定した場合だけTurnへ結ぶ。builtinを含め確定できない結果は `null` としてSession配下に保持し、ヒューリスティックでは埋めない。 |
| `name` | text | 必須 | 正本 Event の `attributes["tool_name"]` | 人間が実行したツールを識別する。 |
| `kind` | text (`builtin` / `mcp` / `unknown`) | 必須 | 正本 Event の `attributes["mcp_tool"]` と `attributes["tool_origin"]` | builtinかMCPかを、実測属性の整合時だけ分類する。欠損・矛盾は `unknown` とする。 |
| `success` | boolean | 必須 | 正本 Event の `attributes["success"]` | ツール終端結果の成否を示す唯一の正本。 |
| `input_bytes` | integer | 必須 | 正本 Event の `attributes["arguments_length"]` | arguments本文を保存せず、入力規模だけを示す。 |
| `output_bytes` | integer | 必須 | 正本 Event の `attributes["output_length"]` | Tool Output本文を保存せず、出力規模だけを示す。 |
| `output_line_count` | integer | 必須 | 正本 Event の `attributes["output_line_count"]` | 出力の概算的な読みやすさを本文なしで示す。 |
| `output_truncated` | boolean | 必須 | 正本 Event の `attributes["output_truncated"]` | 保存しない出力が切り詰められた結果かを示す。 |
| `duration_ms` | integer | 必須 | 正本 Event の `attributes["duration_ms"]` | ToolCall開始時刻は確定できないため、唯一の実測所要時間を保持する。親/MCP Spanのdurationは使わない。 |
| `completed_at` | timestamp | 必須 | 正本 Event の `attributes["event.timestamp"]` | 終端結果の時系列を示す。開始時刻は推測しない。 |

## UnknownTelemetry

UnknownTelemetry は、分類表にないSpan/Eventの仕様変更を検知するための集計である。Rawデータ、
attributes、resource、trace/parent ID、input、outputは保存しない。

| フィールド | 型 | 必須 / nullable | 情報源となるTelemetry | 保存する理由 |
| --- | --- | --- | --- | --- |
| `name` | text | 必須 | 未分類SpanのSpan名、または未分類Eventの `event.name` | 変更・追加されたTelemetry名を識別する。 |
| `kind` | text (`Span` / `Event`) | 必須 | 観測対象の種別 | 同名のSpanとEventを区別し、`name`と組で一意にする。 |
| `count` | integer | 必須 | 同じ `(kind, name)` の観測件数 | Rawを残さず、出現頻度の変化を検知する。 |
| `first_seen` | timestamp | 必須 | 同じ `(kind, name)` の最初のSpan/Event timestamp | 仕様変更を初めて観測した時点を示す。 |
| `last_seen` | timestamp | 必須 | 同じ `(kind, name)` の最後のSpan/Event timestamp | そのTelemetryが最後に観測された時点を示す。 |

## 今回の決定

1. builtin ToolCallでTurnを確定できない場合、`ToolCall.turn_id` は `null` とする。Sessionとの関係は保持し、時刻近接やSpan階層の推測では関連付けない。
2. `cwd` は `Turn.cwd` のnullableな観測値とする。Sessionに単一の `cwd` は持たず、上書き・履歴テーブルも作らない。
3. `attributes["prompt_length"]` は、canonical Turnへ確定的に結べる場合だけ `Turn.prompt_length` に保存する。Prompt本文・配列内容は保存しない。

## 明示的に保存しないもの

Raw Span/Event、User Prompt本文、Tool arguments、Tool Output本文、API payload、認証関連値、
trace/parent ID、resource属性、内部retry・キュー・永続化・WebSocketの詳細時系列は保存しない。
また、Sessionの開始・終了時刻、件数、token合計、Turnのdurationは保存済みの正本レコードから
導出する。
