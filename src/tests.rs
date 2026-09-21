use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest,
    common::v1::{AnyValue, KeyValue, any_value},
    trace::v1::{ResourceSpans, ScopeSpans, Span, span::Event},
};
use prost::Message;
use tower::ServiceExt;

use crate::{http, normalize, store::Database};

#[test]
fn canonical_turn_is_normalized_and_saved_without_derived_duration() {
    let database = database();
    database
        .persist(&normalize::compile(fixture_request()))
        .unwrap();
    let connection = database.connection();
    let turn: (String, i64, i64, i64, String, i64, i64, i64) = connection
        .query_row(
            "SELECT model, input_tokens, output_tokens, total_tokens, cwd, prompt_length, ttft_ms, ended_at - started_at FROM turns WHERE session_id = 'session-1' AND id = 'turn-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)),
        )
        .unwrap();
    assert_eq!(
        turn,
        (
            "gpt-test".into(),
            10,
            20,
            999,
            "/work/project".into(),
            42,
            17,
            30
        )
    );
    let duration_column: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('turns') WHERE name = 'duration_ms'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(duration_column, 0);
}

#[test]
fn tool_calls_are_deduplicated_and_mcp_is_linked_by_explicit_key() {
    let database = database();
    database
        .persist(&normalize::compile(fixture_request()))
        .unwrap();
    let connection = database.connection();
    let tools: Vec<(String, Option<String>, String, i64, i64)> = connection
        .prepare("SELECT id, turn_id, kind, output_bytes, completed_at FROM tool_calls ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(tools.len(), 2);
    assert_eq!(
        tools[0],
        ("builtin-1".into(), None, "builtin".into(), 50, 160)
    );
    assert_eq!(
        tools[1],
        ("mcp-1".into(), Some("turn-1".into()), "mcp".into(), 2, 200)
    );
}

#[test]
fn unknown_telemetry_is_aggregated_and_known_discarded_telemetry_is_not_saved() {
    let database = database();
    database
        .persist(&normalize::compile(fixture_request()))
        .unwrap();
    let connection = database.connection();
    let unknown: (i64, i64, i64) = connection
        .query_row(
            "SELECT count, first_seen, last_seen FROM unknown_telemetry WHERE kind = 'Span' AND name = 'codex.future_span'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(unknown, (2, 300, 400));
    let event_count: i64 = connection
        .query_row(
            "SELECT count FROM unknown_telemetry WHERE kind = 'Event' AND name = 'codex.future_event'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event_count, 2);
    let discarded: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM unknown_telemetry WHERE name IN ('append_items', 'codex.api_request')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(discarded, 0);
}

#[test]
fn database_schema_cannot_store_raw_body_fields() {
    let database = database();
    database
        .persist(&normalize::compile(fixture_request()))
        .unwrap();
    let connection = database.connection();
    let prohibited_columns: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tool_calls') WHERE name IN ('arguments', 'output', 'trace_id', 'parent_id')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(prohibited_columns, 0);
    let raw_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('spans', 'events', 'raw_telemetry')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw_tables, 0);
}

#[tokio::test]
async fn otlp_http_protobuf_decode_normalize_and_sqlite_pipeline() {
    let database = Arc::new(database());
    let request = fixture_request();
    let mut protobuf = Vec::new();
    request.encode(&mut protobuf).unwrap();
    let response = http::router(Arc::clone(&database))
        .oneshot(
            Request::post("/v1/traces")
                .header("content-type", "application/x-protobuf")
                .body(Body::from(protobuf))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let connection = database.connection();
    let sessions: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sessions WHERE id = 'session-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(sessions, 1);
}

fn database() -> Database {
    Database::open(":memory:").unwrap()
}

fn fixture_request() -> ExportTraceServiceRequest {
    let trace = vec![9; 16];
    let spans = vec![
        span(
            trace.clone(),
            vec![1],
            vec![],
            "turn/start",
            100,
            101,
            input(&[("turn.id", string("turn-1"))]),
            vec![],
        ),
        span(
            trace.clone(),
            vec![2],
            vec![1],
            "op.dispatch.turn_input",
            110,
            111,
            vec![],
            vec![event(
                120,
                "event",
                vec![
                    ("event.name", string("codex.user_prompt")),
                    ("conversation.id", string("session-1")),
                    ("prompt_length", integer(42)),
                ],
            )],
        ),
        span(
            trace.clone(),
            vec![3],
            vec![1],
            "session_task.turn",
            130,
            160,
            input(&[(
                "input",
                object(&[
                    ("turn.id", string("turn-1")),
                    ("model", string("gpt-test")),
                    ("codex.turn.reasoning_effort", string("high")),
                    ("codex.turn.token_usage.input_tokens", integer(10)),
                    ("codex.turn.token_usage.cached_input_tokens", integer(5)),
                    ("codex.turn.token_usage.output_tokens", integer(20)),
                    ("codex.turn.token_usage.reasoning_output_tokens", integer(7)),
                    ("codex.turn.token_usage.total_tokens", integer(999)),
                ]),
            )]),
            vec![],
        ),
        span(
            trace.clone(),
            vec![4],
            vec![3],
            "run_sampling_request",
            131,
            132,
            input(&[
                ("turn_id", string("turn-1")),
                ("cwd", string("/work/project")),
                ("model", string("fallback-model")),
            ]),
            vec![],
        ),
        span(
            trace.clone(),
            vec![5],
            vec![3],
            "try_run_sampling_request",
            133,
            134,
            input(&[("turn_id", string("turn-1"))]),
            vec![event(
                135,
                "event",
                vec![
                    ("event.name", string("codex.turn_ttft")),
                    ("duration_ms", integer(17)),
                ],
            )],
        ),
        span(
            trace.clone(),
            vec![6],
            vec![],
            "dispatch_tool_call_with_terminal_outcome",
            140,
            160,
            vec![],
            vec![
                tool_event(150, "builtin-1", "builtin", false, 50),
                tool_event(160, "builtin-1", "builtin", false, 50),
            ],
        ),
        span(
            trace.clone(),
            vec![7],
            vec![1],
            "mcp.tools.call",
            140,
            150,
            input(&[
                ("conversation.id", string("session-1")),
                ("tool.call_id", string("mcp-1")),
                ("turn.id", string("turn-1")),
            ]),
            vec![],
        ),
        span(
            trace.clone(),
            vec![8],
            vec![1],
            "dispatch_tool_call_with_terminal_outcome",
            140,
            200,
            vec![],
            vec![
                tool_event(190, "mcp-1", "mcp", true, 1),
                tool_event(200, "mcp-1", "mcp", true, 2),
                event(
                    210,
                    "event",
                    vec![("event.name", string("codex.future_event"))],
                ),
                event(
                    220,
                    "event",
                    vec![("event.name", string("codex.future_event"))],
                ),
            ],
        ),
        span(
            trace.clone(),
            vec![10],
            vec![],
            "codex.future_span",
            300,
            301,
            vec![],
            vec![],
        ),
        span(
            trace.clone(),
            vec![11],
            vec![],
            "codex.future_span",
            400,
            401,
            vec![],
            vec![],
        ),
        span(
            trace,
            vec![12],
            vec![],
            "append_items",
            500,
            501,
            vec![],
            vec![event(
                500,
                "event",
                vec![("event.name", string("codex.api_request"))],
            )],
        ),
    ];
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

#[allow(clippy::too_many_arguments)] // Test fixture mirrors the OTLP Span fields.
fn span(
    trace_id: Vec<u8>,
    span_id: Vec<u8>,
    parent_span_id: Vec<u8>,
    name: &str,
    started_at: u64,
    ended_at: u64,
    attributes: Vec<KeyValue>,
    events: Vec<Event>,
) -> Span {
    Span {
        trace_id,
        span_id,
        parent_span_id,
        name: name.into(),
        start_time_unix_nano: started_at,
        end_time_unix_nano: ended_at,
        attributes,
        events,
        ..Default::default()
    }
}

fn event(timestamp: u64, name: &str, attributes: Vec<(&str, AnyValue)>) -> Event {
    Event {
        time_unix_nano: timestamp,
        name: name.into(),
        attributes: attributes
            .into_iter()
            .map(|(key, value)| kv(key, value))
            .collect(),
        ..Default::default()
    }
}

fn tool_event(
    timestamp: u64,
    call_id: &str,
    origin: &str,
    is_mcp: bool,
    output_bytes: i64,
) -> Event {
    event(
        timestamp,
        "event",
        vec![
            ("event.name", string("codex.tool_result")),
            ("call_id", string(call_id)),
            ("conversation.id", string("session-1")),
            ("tool_name", string("safe_tool")),
            ("mcp_tool", boolean(is_mcp)),
            ("tool_origin", string(origin)),
            ("success", boolean(true)),
            ("arguments_length", integer(11)),
            ("output_length", integer(output_bytes)),
            ("output_line_count", integer(2)),
            ("output_truncated", boolean(false)),
            ("duration_ms", integer(13)),
        ],
    )
}

fn input(values: &[(&str, AnyValue)]) -> Vec<KeyValue> {
    values
        .iter()
        .map(|(key, value)| kv(key, value.clone()))
        .collect()
}

fn kv(key: &str, value: AnyValue) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(value),
    }
}

fn string(value: &str) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.into())),
    }
}

fn integer(value: i64) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::IntValue(value)),
    }
}

fn boolean(value: bool) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::BoolValue(value)),
    }
}

fn object(values: &[(&str, AnyValue)]) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::KvlistValue(
            opentelemetry_proto::tonic::common::v1::KeyValueList {
                values: input(values),
            },
        )),
    }
}
