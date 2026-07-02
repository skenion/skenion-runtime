use std::convert::Infallible;

use axum::{
    Json,
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
};
use tokio_stream::{
    Stream, StreamExt,
    wrappers::{BroadcastStream, errors::BroadcastStreamRecvError},
};

use crate::RuntimeLogSnapshotResponse;

use super::RuntimeServerState;

pub(super) async fn runtime_logs(
    State(state): State<RuntimeServerState>,
) -> Json<RuntimeLogSnapshotResponse> {
    Json(state.logs.snapshot())
}

pub(super) async fn runtime_logs_stream(
    State(state): State<RuntimeServerState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.logs.subscribe();
    let replay = tokio_stream::iter(
        state
            .logs
            .snapshot()
            .events
            .into_iter()
            .map(runtime_log_event),
    );
    let live = BroadcastStream::new(receiver).map(runtime_log_broadcast_event);
    Sse::new(replay.chain(live)).keep_alive(KeepAlive::default())
}

fn runtime_log_broadcast_event(
    result: Result<crate::RuntimeLogEvent, BroadcastStreamRecvError>,
) -> Result<Event, Infallible> {
    match result {
        Ok(event) => runtime_log_event(event),
        Err(_) => Ok(Event::default()
            .event("log-gap")
            .data("runtime log stream receiver lagged")),
    }
}

fn runtime_log_event(event: crate::RuntimeLogEvent) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("log")
        .json_data(event)
        .expect("runtime log event should serialize"))
}
