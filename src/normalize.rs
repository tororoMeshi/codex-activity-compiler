use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest,
    common::v1::{AnyValue, KeyValue, any_value},
    trace::v1::Span,
};
use serde_json::Value;

#[derive(Debug, Default)]
pub struct CompiledTelemetry {
    pub sessions: BTreeSet<String>,
    pub turns: Vec<Turn>,
    pub tool_calls: Vec<ToolCall>,
    pub unknown: Vec<UnknownTelemetry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub id: String,
    pub session_id: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub started_at: i64,
    pub ended_at: i64,
    pub ttft_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cwd: Option<String>,
    pub prompt_length: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub name: String,
    pub kind: ToolKind,
    pub success: bool,
    pub input_bytes: i64,
    pub output_bytes: i64,
    pub output_line_count: i64,
    pub output_truncated: bool,
    pub duration_ms: i64,
    pub completed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Builtin,
    Mcp,
    Unknown,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Mcp => "mcp",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTelemetry {
    pub name: String,
    pub kind: TelemetryKind,
    pub count: i64,
    pub first_seen: i64,
    pub last_seen: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryKind {
    Span,
    Event,
}

impl TelemetryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Span => "Span",
            Self::Event => "Event",
        }
    }
}

#[derive(Debug)]
struct RawSpan {
    trace_id: String,
    span_id: String,
    parent_span_id: String,
    name: String,
    started_at: i64,
    ended_at: i64,
    attributes: BTreeMap<String, Value>,
    events: Vec<RawEvent>,
}

#[derive(Debug)]
struct RawEvent {
    name: String,
    timestamp: i64,
    attributes: BTreeMap<String, Value>,
}

#[derive(Debug, Default)]
struct TurnExtras {
    model: Option<String>,
    cwd: Option<String>,
    ttft_ms: Option<i64>,
    prompt_length: Option<i64>,
}

/// Converts an OTLP request in memory. Raw identifiers and attributes do not leave this function.
pub fn compile(request: ExportTraceServiceRequest) -> CompiledTelemetry {
    let spans = raw_spans(request);
    let parents = parent_map(&spans);
    let start_turn_ids = turn_start_ids(&spans);
    let sessions_by_turn = sessions_by_turn(&spans, &parents, &start_turn_ids);
    let mut extras_by_turn = extras_by_turn(&spans);
    for (turn_id, prompt_length) in prompt_lengths_by_turn(&spans, &parents, &start_turn_ids) {
        extras_by_turn.entry(turn_id).or_default().prompt_length = Some(prompt_length);
    }

    let mut compiled = CompiledTelemetry::default();
    let mut canonical_turns = HashMap::new();
    for raw in spans.iter().filter(|span| span.name == "session_task.turn") {
        let Some(turn_id) = input_string(raw, "turn.id") else {
            continue;
        };
        let Some(session_id) = sessions_by_turn.get(&turn_id).cloned() else {
            // `thread.id` alone is deliberately not a Session key.
            continue;
        };
        let extras = extras_by_turn.get(&turn_id);
        let canonical_model = input_string(raw, "model");
        let turn = Turn {
            id: turn_id.clone(),
            session_id: session_id.clone(),
            model: canonical_model.or_else(|| extras.and_then(|extra| extra.model.clone())),
            reasoning_effort: input_string(raw, "codex.turn.reasoning_effort"),
            started_at: raw.started_at,
            ended_at: raw.ended_at,
            ttft_ms: extras.and_then(|extra| extra.ttft_ms),
            input_tokens: input_i64(raw, "codex.turn.token_usage.input_tokens"),
            cached_input_tokens: input_i64(raw, "codex.turn.token_usage.cached_input_tokens"),
            output_tokens: input_i64(raw, "codex.turn.token_usage.output_tokens"),
            reasoning_output_tokens: input_i64(
                raw,
                "codex.turn.token_usage.reasoning_output_tokens",
            ),
            total_tokens: input_i64(raw, "codex.turn.token_usage.total_tokens"),
            cwd: extras.and_then(|extra| extra.cwd.clone()),
            prompt_length: extras.and_then(|extra| extra.prompt_length),
        };
        compiled.sessions.insert(session_id);
        canonical_turns.insert(
            (turn.session_id.clone(), turn.id.clone()),
            raw.span_id.clone(),
        );
        compiled.turns.push(turn);
    }

    let mcp_links = mcp_links(&spans);
    let mut tools_by_key: HashMap<(String, String), ToolCall> = HashMap::new();
    let mut unknown = UnknownAccumulator::default();
    for raw in &spans {
        if !known_span(&raw.name) {
            unknown.record(&raw.name, TelemetryKind::Span, raw.started_at);
        }
        for event in &raw.events {
            let event_name = event_identity(event);
            match event_name.as_str() {
                "codex.tool_result" => {
                    if let Some(mut tool) = tool_call(raw, event) {
                        compiled.sessions.insert(tool.session_id.clone());
                        tool.turn_id = resolve_tool_turn(
                            raw,
                            &tool,
                            &mcp_links,
                            &parents,
                            &start_turn_ids,
                            &canonical_turns,
                        );
                        let key = (tool.session_id.clone(), tool.id.clone());
                        match tools_by_key.get(&key) {
                            Some(existing) if existing.completed_at >= tool.completed_at => {}
                            _ => {
                                tools_by_key.insert(key, tool);
                            }
                        }
                    }
                }
                "codex.turn_ttft" | "codex.user_prompt" => {}
                name if discarded_event(name) => {}
                name => unknown.record(name, TelemetryKind::Event, event.timestamp),
            }
        }
    }
    compiled.tool_calls = tools_by_key.into_values().collect();
    compiled.tool_calls.sort_by(|a, b| {
        (&a.session_id, &a.id, a.completed_at).cmp(&(&b.session_id, &b.id, b.completed_at))
    });
    compiled
        .turns
        .sort_by(|a, b| (&a.session_id, &a.id).cmp(&(&b.session_id, &b.id)));
    compiled.unknown = unknown.finish();
    compiled
}

fn raw_spans(request: ExportTraceServiceRequest) -> Vec<RawSpan> {
    request
        .resource_spans
        .into_iter()
        .flat_map(|resource| resource.scope_spans)
        .flat_map(|scope| scope.spans)
        .map(raw_span)
        .collect()
}

fn raw_span(span: Span) -> RawSpan {
    RawSpan {
        trace_id: hex(&span.trace_id),
        span_id: hex(&span.span_id),
        parent_span_id: hex(&span.parent_span_id),
        name: span.name,
        started_at: nanos_to_i64(span.start_time_unix_nano),
        ended_at: nanos_to_i64(span.end_time_unix_nano),
        attributes: attributes(span.attributes),
        events: span
            .events
            .into_iter()
            .map(|event| RawEvent {
                name: event.name,
                timestamp: nanos_to_i64(event.time_unix_nano),
                attributes: attributes(event.attributes),
            })
            .collect(),
    }
}

fn nanos_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn attributes(values: Vec<KeyValue>) -> BTreeMap<String, Value> {
    values
        .into_iter()
        .filter_map(|attribute| {
            let value = attribute.value.and_then(any_value_to_json)?;
            Some((attribute.key, value))
        })
        .collect()
}

fn any_value_to_json(value: AnyValue) -> Option<Value> {
    match value.value? {
        any_value::Value::StringValue(value) => Some(Value::String(value)),
        any_value::Value::BoolValue(value) => Some(Value::Bool(value)),
        any_value::Value::IntValue(value) => Some(Value::Number(value.into())),
        any_value::Value::DoubleValue(value) => {
            serde_json::Number::from_f64(value).map(Value::Number)
        }
        any_value::Value::BytesValue(_) => None,
        any_value::Value::ArrayValue(values) => Some(Value::Array(
            values
                .values
                .into_iter()
                .filter_map(any_value_to_json)
                .collect(),
        )),
        any_value::Value::KvlistValue(values) => Some(Value::Object(
            attributes(values.values)
                .into_iter()
                .collect::<serde_json::Map<_, _>>(),
        )),
    }
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn parent_map(spans: &[RawSpan]) -> HashMap<(String, String), String> {
    spans
        .iter()
        .filter(|span| !span.parent_span_id.is_empty())
        .map(|span| {
            (
                (span.trace_id.clone(), span.span_id.clone()),
                span.parent_span_id.clone(),
            )
        })
        .collect()
}

fn turn_start_ids(spans: &[RawSpan]) -> HashMap<(String, String), String> {
    spans
        .iter()
        .filter(|span| span.name == "turn/start")
        .filter_map(|span| {
            input_string(span, "turn.id")
                .map(|turn_id| ((span.trace_id.clone(), span.span_id.clone()), turn_id))
        })
        .collect()
}

fn sessions_by_turn(
    spans: &[RawSpan],
    parents: &HashMap<(String, String), String>,
    start_turn_ids: &HashMap<(String, String), String>,
) -> HashMap<String, String> {
    let mut candidates: HashMap<String, BTreeSet<String>> = HashMap::new();
    for span in spans {
        for event in &span.events {
            if event_identity(event) != "codex.user_prompt" {
                continue;
            }
            let Some(session_id) = string_attr(&event.attributes, "conversation.id") else {
                continue;
            };
            if let Some(turn_id) = ancestor_turn_id(span, parents, start_turn_ids) {
                candidates.entry(turn_id).or_default().insert(session_id);
            }
        }
    }
    candidates
        .into_iter()
        .filter_map(|(turn_id, sessions)| {
            (sessions.len() == 1)
                .then(|| (turn_id, sessions.into_iter().next().expect("one value")))
        })
        .collect()
}

fn extras_by_turn(spans: &[RawSpan]) -> HashMap<String, TurnExtras> {
    let mut extras = HashMap::<String, TurnExtras>::new();
    for span in spans {
        match span.name.as_str() {
            "run_sampling_request" => {
                if let Some(turn_id) = input_string(span, "turn_id") {
                    let entry = extras.entry(turn_id).or_default();
                    if entry.cwd.is_none() {
                        entry.cwd = input_string(span, "cwd");
                    }
                    if entry.model.is_none() {
                        entry.model = input_string(span, "model");
                    }
                }
            }
            "try_run_sampling_request" => {
                if let Some(turn_id) = input_string(span, "turn_id") {
                    for event in &span.events {
                        if event_identity(event) == "codex.turn_ttft" {
                            extras.entry(turn_id.clone()).or_default().ttft_ms =
                                i64_attr(&event.attributes, "duration_ms");
                        }
                    }
                }
            }
            _ => {}
        }
    }
    extras
}

fn prompt_lengths_by_turn(
    spans: &[RawSpan],
    parents: &HashMap<(String, String), String>,
    start_turn_ids: &HashMap<(String, String), String>,
) -> HashMap<String, i64> {
    let mut candidates: HashMap<String, BTreeSet<i64>> = HashMap::new();
    for span in spans {
        let Some(turn_id) = ancestor_turn_id(span, parents, start_turn_ids) else {
            continue;
        };
        for event in &span.events {
            if event_identity(event) == "codex.user_prompt"
                && let Some(prompt_length) = i64_attr(&event.attributes, "prompt_length")
            {
                candidates
                    .entry(turn_id.clone())
                    .or_default()
                    .insert(prompt_length);
            }
        }
    }
    candidates
        .into_iter()
        .filter_map(|(turn_id, values)| {
            (values.len() == 1).then(|| (turn_id, values.into_iter().next().expect("one value")))
        })
        .collect()
}

fn mcp_links(spans: &[RawSpan]) -> HashMap<(String, String), String> {
    spans
        .iter()
        .filter(|span| span.name == "mcp.tools.call")
        .filter_map(|span| {
            let session_id = input_string(span, "conversation.id")?;
            let call_id = input_string(span, "tool.call_id")?;
            let turn_id = input_string(span, "turn.id")?;
            Some(((session_id, call_id), turn_id))
        })
        .collect()
}

fn resolve_tool_turn(
    span: &RawSpan,
    tool: &ToolCall,
    mcp_links: &HashMap<(String, String), String>,
    parents: &HashMap<(String, String), String>,
    start_turn_ids: &HashMap<(String, String), String>,
    canonical_turns: &HashMap<(String, String), String>,
) -> Option<String> {
    if let Some(turn_id) = mcp_links.get(&(tool.session_id.clone(), tool.id.clone())) {
        return Some(turn_id.clone());
    }
    let turn_id = ancestor_turn_id(span, parents, start_turn_ids)?;
    canonical_turns
        .contains_key(&(tool.session_id.clone(), turn_id.clone()))
        .then_some(turn_id)
}

fn ancestor_turn_id(
    span: &RawSpan,
    parents: &HashMap<(String, String), String>,
    start_turn_ids: &HashMap<(String, String), String>,
) -> Option<String> {
    let mut span_id = span.span_id.clone();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(span_id.clone()) {
            return None;
        }
        if let Some(turn_id) = start_turn_ids.get(&(span.trace_id.clone(), span_id.clone())) {
            return Some(turn_id.clone());
        }
        let parent = parents.get(&(span.trace_id.clone(), span_id.clone()))?;
        span_id = parent.clone();
    }
}

fn tool_call(_span: &RawSpan, event: &RawEvent) -> Option<ToolCall> {
    let attributes = &event.attributes;
    Some(ToolCall {
        id: string_attr(attributes, "call_id")?,
        session_id: string_attr(attributes, "conversation.id")?,
        turn_id: None,
        name: string_attr(attributes, "tool_name")?,
        kind: match (
            bool_attr(attributes, "mcp_tool"),
            string_attr(attributes, "tool_origin"),
        ) {
            (Some(true), Some(origin)) if origin == "mcp" => ToolKind::Mcp,
            (Some(false), Some(origin)) if origin == "builtin" => ToolKind::Builtin,
            _ => ToolKind::Unknown,
        },
        success: bool_attr(attributes, "success")?,
        input_bytes: i64_attr(attributes, "arguments_length")?,
        output_bytes: i64_attr(attributes, "output_length")?,
        output_line_count: i64_attr(attributes, "output_line_count")?,
        output_truncated: bool_attr(attributes, "output_truncated")?,
        duration_ms: i64_attr(attributes, "duration_ms")?,
        completed_at: event.timestamp,
    })
}

fn known_span(name: &str) -> bool {
    matches!(
        name,
        "session_task.turn"
            | "run_sampling_request"
            | "try_run_sampling_request"
            | "op.dispatch.turn_input"
            | "turn/start"
            | "dispatch_tool_call_with_terminal_outcome"
            | "mcp.tools.call"
            | "handle_responses"
            | "append_items"
            | "persist_rollout_items"
            | "realtime_conversation.running_state"
            | "auth"
            | "responses_websocket.stream_request"
            | "endpoint_session.execute_with"
            | "startup_prewarm.resolve"
            | "thread/start"
    )
}

fn discarded_event(name: &str) -> bool {
    matches!(
        name,
        "codex.api_request" | "codex.startup_phase" | "codex.websocket_request"
    )
}

fn event_identity(event: &RawEvent) -> String {
    string_attr(&event.attributes, "event.name").unwrap_or_else(|| event.name.clone())
}

fn input_value(span: &RawSpan, key: &str) -> Option<Value> {
    if let Some(Value::String(input)) = span.attributes.get("input")
        && let Ok(Value::Object(input)) = serde_json::from_str(input)
        && let Some(value) = input.get(key)
    {
        return Some(value.clone());
    }
    if let Some(Value::Object(input)) = span.attributes.get("input")
        && let Some(value) = input.get(key)
    {
        return Some(value.clone());
    }
    span.attributes
        .get(&format!("input.{key}"))
        .or_else(|| span.attributes.get(key))
        .cloned()
}

fn input_string(span: &RawSpan, key: &str) -> Option<String> {
    value_to_string(input_value(span, key)?)
}

fn input_i64(span: &RawSpan, key: &str) -> Option<i64> {
    value_to_i64(input_value(span, key)?)
}

fn string_attr(attributes: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    value_to_string(attributes.get(key)?.clone())
}

fn i64_attr(attributes: &BTreeMap<String, Value>, key: &str) -> Option<i64> {
    value_to_i64(attributes.get(key)?.clone())
}

fn bool_attr(attributes: &BTreeMap<String, Value>, key: &str) -> Option<bool> {
    match attributes.get(key)? {
        Value::Bool(value) => Some(*value),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn value_to_string(value: Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_to_i64(value: Value) -> Option<i64> {
    match value {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

#[derive(Default)]
struct UnknownAccumulator(HashMap<(String, String), UnknownTelemetry>);

impl UnknownAccumulator {
    fn record(&mut self, name: &str, kind: TelemetryKind, timestamp: i64) {
        let key = (kind.as_str().to_owned(), name.to_owned());
        self.0
            .entry(key)
            .and_modify(|entry| {
                entry.count += 1;
                entry.first_seen = entry.first_seen.min(timestamp);
                entry.last_seen = entry.last_seen.max(timestamp);
            })
            .or_insert_with(|| UnknownTelemetry {
                name: name.to_owned(),
                kind,
                count: 1,
                first_seen: timestamp,
                last_seen: timestamp,
            });
    }

    fn finish(self) -> Vec<UnknownTelemetry> {
        let mut values: Vec<_> = self.0.into_values().collect();
        values.sort_by(|a, b| (a.kind.as_str(), &a.name).cmp(&(b.kind.as_str(), &b.name)));
        values
    }
}
