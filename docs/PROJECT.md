# Codex Activity Compiler — Project Design Brief

## 1. 目的

**Codexが出力する大量のOTLPデータから、人間が開発作業を理解・振り返るために必要な情報だけを抽出し、低リソースで記録・表示する。**

OTLPそのものの保存や汎用的なトレース解析は目的としない。

人間が最終的に、

```text
Session
  └─ Turn
       └─ ToolCall
```

の単位でCodexの活動を理解できる状態を作る。

## 2. 背景

CodexはOpenTelemetryで詳細な実行情報を出力できる。

実際のOTLPには、モデル、token使用量、reasoning effort、Turn所要時間、ToolCallの成功可否、MCP判定、作業ディレクトリなど、開発作業の振り返りに有用な情報が含まれている。`session_task.turn` にはTurn単位のtoken情報やmodel、reasoning effortがまとまっている。

一方で、内部実装上の細かなSpanも大量に出力される。

## 3. 現状の問題

実測では `append_items` 582件、`persist_rollout_items` 326件、`realtime_conversation.running_state` 219件、`auth` 113件など、人間が通常見る必要のない内部Spanが大量に発生している。

そのため、OTLPをそのまま保存する方式では、データ量とストレージ消費が大きくなり、必要な情報がノイズに埋もれる。

また、汎用Observability基盤では「何が起きたか」は記録できても、「Codexが開発作業として何をしたか」を理解するには粒度が細かすぎる。

## 4. 提案方式

### アルゴリズム

受信したOTLPをRaw Spanとして保存せず、受信時に意味判定して人間向けの活動データへ変換する。

```text
OTLP受信
   ↓
Span / Event識別
   ↓
意味分類
   ├─ Session情報
   ├─ Turn情報
   ├─ ToolCall情報
   ├─ 集計情報
   └─ 不要情報
   ↓
必要属性だけ抽出
   ↓
重複情報を統合
   ↓
Session / Turn / ToolCallへ正規化
   ↓
Raw Span破棄
```

判断単位はSpan名だけではなく、Span内部の `opentelemetry.events` も使用する。

例えば `dispatch_tool_call_with_terminal_outcome` そのものではなく、その内部にある `codex.tool_result` からtool名、成功可否、duration、MCP判定などを抽出する。

Turnについては `session_task.turn` を基準レコードとし、別Spanに存在するTTFTや作業ディレクトリなどをTurnへ統合する。TTFTは `codex.turn_ttft`、作業ディレクトリは `run_sampling_request` から取得できる。

### 他にない工夫点

**保存前に意味へ変換する。**

一般的なObservability製品は、

```text
収集 → 保存 → 検索 → 人間が意味を判断
```

する。

本方式では、

```text
収集 → 意味判断 → 圧縮 → 保存
```

とする。

そのため、数千〜数万のRaw Spanを保存せず、人間に必要な数十件程度の活動データへ縮約できる。

また、既知でないSpan/EventについてはRawデータを保存せず、

```text
name
kind
count
first_seen
last_seen
```

のみ保持する。

これにより、保存容量を増やさずにCodexのバージョンアップによるTelemetry仕様変更を検知できる。

もう一つの特徴は、**Codex内部実装ではなく開発活動をデータモデルの中心に置くこと**である。

Codex内部のRust関数名やSpan階層が変わっても、最終的な保存形式は、

```text
Session
Turn
ToolCall
```

を維持する。

## 5. 実装・設計

実装言語はRustとする。

単一プロセスを基本とし、外部DB、Redis、メッセージキューなどを必要としない。

```text
Rust Application
├─ OTLP/HTTP Receiver
├─ Codex Adapter
├─ Normalizer
├─ Aggregator
├─ SQLite
└─ Web UI（将来構成。今回のMVPコア実装範囲外）
```

今回のMVPコアでは、受信したTelemetryをRawのまま保存せず、次の4種類へ正規化してSQLiteへ保存する。

| データ      | 主な内容                                               |
| -------- | -------------------------------------------------- |
| Session  | conversation ID |
| Turn     | model、reasoning effort、token内訳、開始・終了時刻、TTFT、cwd、prompt長 |
| ToolCall | tool名、builtin/MCP、成功可否、duration、入出力量               |
| UnknownTelemetry | 未分類Span/Eventのname、kind、count、初回・最終観測時刻 |

Sessionの開始・終了時刻、Turn数、ToolCall数、token合計は下位レコードから導出し、永続化しない。Turn durationも開始・終了時刻から導出し、永続化しない。

ユーザープロンプト本文やTool出力本文はMVPコアでは保存しない。`codex.user_prompt` からは、canonical Turnへ確定的に関連付けられるprompt長だけを利用する。

Web UIは将来、Session一覧からTurn、ToolCallへ辿れる最低限の構成とする。現在実装済みのMVPコアは、OTLP/HTTP受信、正規化、SQLite保存までである。

## 6. 評価方式

MVPでは、次の観点で評価する。

**情報削減率**
受信したRaw Span数に対して、保存レコード数と保存容量をどこまで削減できるか。

**情報保持率**
model、token、duration、TTFT、ToolCall、成功失敗、cwdなど、人間が必要とする情報を失っていないか。

**可読性**
Raw Traceを確認せずに、「どのプロジェクトで、どのモデルが、どれだけtokenを使い、何を実行し、どこで失敗したか」を理解できるか。

**軽量性**
常時稼働時のCPU、メモリ、ストレージ消費が、既存の開発環境に対して無視できる程度か。

**耐変更性**
未知Span/Eventを検知し、Codex更新によるTelemetry変更に追従できるか。
