use std::{convert::Infallible, time::Duration};

use axum::{
    Json,
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
};
use tokio_stream::{Stream, StreamExt, wrappers::IntervalStream};

use crate::{RuntimeSessionRecord, RuntimeTelemetrySnapshot};

use super::RuntimeServerState;

pub(super) async fn session_telemetry_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Json<RuntimeTelemetrySnapshot> {
    Json(telemetry_snapshot(
        &state,
        state.sessions.get_or_create(&session_id),
    ))
}

pub(super) async fn session_telemetry_stream_by_id(
    State(state): State<RuntimeServerState>,
    Path(session_id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    session_telemetry_stream_for(state, session_id)
}

fn session_telemetry_stream_for(
    state: RuntimeServerState,
    session_id: String,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream =
        IntervalStream::new(tokio::time::interval(Duration::from_millis(1000))).map(move |_| {
            let record = state.sessions.get_or_create(&session_id);
            let event = Event::default()
                .event("telemetry")
                .json_data(telemetry_snapshot(&state, record))
                .expect("telemetry snapshot should serialize");
            Ok(event)
        });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn telemetry_snapshot(
    state: &RuntimeServerState,
    record: RuntimeSessionRecord,
) -> RuntimeTelemetrySnapshot {
    let snapshot = {
        let session = record
            .session
            .read()
            .expect("runtime session lock should not be poisoned");
        session.snapshot()
    };
    let mut preview = record
        .preview
        .lock()
        .expect("runtime preview lock should not be poisoned");
    preview.telemetry(
        snapshot,
        state
            .started_at
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    )
}
