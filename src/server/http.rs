use axum::http::StatusCode;
use axum::response::IntoResponse;

pub async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}
