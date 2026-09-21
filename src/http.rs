use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    http::{HeaderValue, StatusCode, header},
    response::Response,
    routing::post,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use prost::Message;

use crate::{normalize, store::Database};

pub fn router(database: Arc<Database>) -> Router {
    Router::new()
        .route("/v1/traces", post(receive_traces))
        .with_state(database)
}

async fn receive_traces(
    axum::extract::State(database): axum::extract::State<Arc<Database>>,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    let request = ExportTraceServiceRequest::decode(body).map_err(|error| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid OTLP protobuf: {error}"),
        )
    })?;
    let compiled = normalize::compile(request);
    database
        .persist(&compiled)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    let mut response_body = Vec::new();
    ExportTraceServiceResponse::default()
        .encode(&mut response_body)
        .expect("encode empty OTLP response");
    let mut response = Response::new(response_body.into());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-protobuf"),
    );
    Ok(response)
}
