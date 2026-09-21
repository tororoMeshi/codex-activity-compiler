use std::{path::Path, sync::Mutex};

use rusqlite::{Connection, Result, Transaction, params};

use crate::normalize::CompiledTelemetry;

/// A single-file SQLite store. It intentionally has no table for raw OTLP spans or events.
pub struct Database {
    connection: Mutex<Connection>,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY NOT NULL
            );

            CREATE TABLE IF NOT EXISTS turns (
                id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                model TEXT,
                reasoning_effort TEXT,
                started_at INTEGER NOT NULL,
                ended_at INTEGER NOT NULL,
                ttft_ms INTEGER,
                input_tokens INTEGER,
                cached_input_tokens INTEGER,
                output_tokens INTEGER,
                reasoning_output_tokens INTEGER,
                total_tokens INTEGER,
                cwd TEXT,
                prompt_length INTEGER,
                PRIMARY KEY (session_id, id),
                FOREIGN KEY (session_id) REFERENCES sessions(id)
            );

            CREATE TABLE IF NOT EXISTS tool_calls (
                id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                turn_id TEXT,
                name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('builtin', 'mcp', 'unknown')),
                success INTEGER NOT NULL CHECK (success IN (0, 1)),
                input_bytes INTEGER NOT NULL,
                output_bytes INTEGER NOT NULL,
                output_line_count INTEGER NOT NULL,
                output_truncated INTEGER NOT NULL CHECK (output_truncated IN (0, 1)),
                duration_ms INTEGER NOT NULL,
                completed_at INTEGER NOT NULL,
                PRIMARY KEY (session_id, id),
                FOREIGN KEY (session_id) REFERENCES sessions(id),
                FOREIGN KEY (session_id, turn_id) REFERENCES turns(session_id, id)
            );

            CREATE TABLE IF NOT EXISTS unknown_telemetry (
                name TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('Span', 'Event')),
                count INTEGER NOT NULL CHECK (count > 0),
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                PRIMARY KEY (kind, name)
            );
            ",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn persist(&self, compiled: &CompiledTelemetry) -> Result<()> {
        let mut connection = self.connection.lock().expect("database mutex poisoned");
        let transaction = connection.transaction()?;
        persist_transaction(&transaction, compiled)?;
        transaction.commit()
    }

    #[cfg(test)]
    pub(crate) fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection.lock().expect("database mutex poisoned")
    }
}

fn persist_transaction(transaction: &Transaction<'_>, compiled: &CompiledTelemetry) -> Result<()> {
    for session_id in &compiled.sessions {
        transaction.execute(
            "INSERT INTO sessions (id) VALUES (?1) ON CONFLICT(id) DO NOTHING",
            [session_id],
        )?;
    }
    for turn in &compiled.turns {
        transaction.execute(
            "
            INSERT INTO turns (
                id, session_id, model, reasoning_effort, started_at, ended_at, ttft_ms,
                input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens,
                total_tokens, cwd, prompt_length
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
            )
            ON CONFLICT(session_id, id) DO UPDATE SET
                model = excluded.model,
                reasoning_effort = excluded.reasoning_effort,
                started_at = excluded.started_at,
                ended_at = excluded.ended_at,
                ttft_ms = excluded.ttft_ms,
                input_tokens = excluded.input_tokens,
                cached_input_tokens = excluded.cached_input_tokens,
                output_tokens = excluded.output_tokens,
                reasoning_output_tokens = excluded.reasoning_output_tokens,
                total_tokens = excluded.total_tokens,
                cwd = excluded.cwd,
                prompt_length = excluded.prompt_length
            ",
            params![
                turn.id,
                turn.session_id,
                turn.model,
                turn.reasoning_effort,
                turn.started_at,
                turn.ended_at,
                turn.ttft_ms,
                turn.input_tokens,
                turn.cached_input_tokens,
                turn.output_tokens,
                turn.reasoning_output_tokens,
                turn.total_tokens,
                turn.cwd,
                turn.prompt_length,
            ],
        )?;
    }
    for tool in &compiled.tool_calls {
        transaction.execute(
            "
            INSERT INTO tool_calls (
                id, session_id, turn_id, name, kind, success, input_bytes, output_bytes,
                output_line_count, output_truncated, duration_ms, completed_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            ON CONFLICT(session_id, id) DO UPDATE SET
                turn_id = COALESCE(excluded.turn_id, tool_calls.turn_id),
                name = excluded.name,
                kind = excluded.kind,
                success = excluded.success,
                input_bytes = excluded.input_bytes,
                output_bytes = excluded.output_bytes,
                output_line_count = excluded.output_line_count,
                output_truncated = excluded.output_truncated,
                duration_ms = excluded.duration_ms,
                completed_at = excluded.completed_at
            WHERE excluded.completed_at > tool_calls.completed_at
            ",
            params![
                tool.id,
                tool.session_id,
                tool.turn_id,
                tool.name,
                tool.kind.as_str(),
                i64::from(tool.success),
                tool.input_bytes,
                tool.output_bytes,
                tool.output_line_count,
                i64::from(tool.output_truncated),
                tool.duration_ms,
                tool.completed_at,
            ],
        )?;
    }
    for unknown in &compiled.unknown {
        transaction.execute(
            "
            INSERT INTO unknown_telemetry (name, kind, count, first_seen, last_seen)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(kind, name) DO UPDATE SET
                count = unknown_telemetry.count + excluded.count,
                first_seen = MIN(unknown_telemetry.first_seen, excluded.first_seen),
                last_seen = MAX(unknown_telemetry.last_seen, excluded.last_seen)
            ",
            params![
                unknown.name,
                unknown.kind.as_str(),
                unknown.count,
                unknown.first_seen,
                unknown.last_seen,
            ],
        )?;
    }
    Ok(())
}
