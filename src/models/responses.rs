use axum::response::{IntoResponse, Response};
use axum::http::{header, StatusCode};

pub struct TileResponse {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

impl IntoResponse for TileResponse {
    fn into_response(self) -> Response {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, self.content_type)],
            self.bytes,
        )
            .into_response()
    }
}
