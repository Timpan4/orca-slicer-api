use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub message: String,
    pub details: Option<String>,
}
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Bad(String),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Execution(String),
    #[error("internal error")]
    Internal,
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Bad(_) => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Execution(_) | Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = match &self {
            Self::Bad(_) => "Invalid slicer request",
            Self::NotFound => "Not found",
            Self::Unavailable(_) => "Slicer unavailable",
            Self::Conflict(_) => "Slicer schema mismatch",
            Self::Execution(_) | Self::Internal => "Failed to slice the model",
        };
        (
            status,
            Json(ErrorBody {
                details: match &self {
                    Self::Bad(details)
                    | Self::Unavailable(details)
                    | Self::Conflict(details)
                    | Self::Execution(details) => Some(details.clone()),
                    _ => None,
                },
                message: message.into(),
            }),
        )
            .into_response()
    }
}
impl From<std::io::Error> for AppError {
    fn from(_: std::io::Error) -> Self {
        Self::Internal
    }
}
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::Bad(e.to_string())
    }
}
