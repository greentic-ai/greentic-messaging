use std::time::Duration;

use anyhow::Error;
use axum::response::{IntoResponse, Response};
use axum::{Json, http::StatusCode};
use serde::Serialize;

use crate::directline_client::DirectLineError;

#[derive(Debug, thiserror::Error)]
pub enum WebChatError {
    #[error("direct line secret not configured for tenant")]
    MissingSecret,
    #[error("invalid request: {0}")]
    BadRequest(&'static str),
    #[error("resource not found: {0}")]
    NotFound(&'static str),
    #[error("direct line error")]
    DirectLine(#[from] DirectLineError),
    #[error("internal server error")]
    Internal(#[source] Error),
}

impl WebChatError {
    pub fn status(&self) -> StatusCode {
        match self {
            WebChatError::MissingSecret => StatusCode::INTERNAL_SERVER_ERROR,
            WebChatError::BadRequest(_) => StatusCode::BAD_REQUEST,
            WebChatError::NotFound(_) => StatusCode::NOT_FOUND,
            WebChatError::DirectLine(DirectLineError::Remote { status, .. }) => {
                if status.is_server_error() {
                    StatusCode::BAD_GATEWAY
                } else {
                    StatusCode::BAD_REQUEST
                }
            }
            WebChatError::DirectLine(_) => StatusCode::BAD_GATEWAY,
            WebChatError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            WebChatError::DirectLine(DirectLineError::Remote { retry_after, .. }) => *retry_after,
            _ => None,
        }
    }

    fn message(&self) -> String {
        match self {
            WebChatError::DirectLine(DirectLineError::Remote { .. }) => {
                "Downstream Direct Line error".to_string()
            }
            WebChatError::NotFound(message) => (*message).to_string(),
            WebChatError::Internal(_) => "internal server error".to_string(),
            other => other.to_string(),
        }
    }
}

impl IntoResponse for WebChatError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(ErrorBody {
            error: self.message(),
        });
        let mut response = (status, body).into_response();
        if let Some(retry_after) = self.retry_after() {
            if let Ok(header) =
                axum::http::HeaderValue::from_str(&retry_after.as_secs().to_string())
            {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, header);
            }
        }
        response
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}
