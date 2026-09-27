//! Subscription types and error definitions

use serde::{Deserialize, Serialize};

/// Subscription error types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubscriptionError {
    /// No credentials configured for this provider
    NoCredentials,
    /// Token has expired and cannot be refreshed
    TokenExpired,
    /// No refresh token available
    NoRefreshToken,
    /// Token refresh failed
    RefreshFailed { message: String },
    /// API request failed
    ApiError { status: u16, message: String },
    /// Response parsing failed
    ParseError { message: String },
    /// Network error
    NetworkError { message: String },
    /// Rate limited
    RateLimited { retry_after: Option<u64> },
}

impl SubscriptionError {
    /// Convert to user-friendly message
    pub fn user_message(&self) -> String {
        match self {
            Self::NoCredentials => "No credentials configured".to_string(),
            Self::TokenExpired => "Session expired, please re-authenticate".to_string(),
            Self::NoRefreshToken => "No refresh token available".to_string(),
            Self::RefreshFailed { message } => format!("Token refresh failed: {}", message),
            Self::ApiError { status, message } => format!("API error ({}): {}", status, message),
            Self::ParseError { message } => format!("Failed to parse response: {}", message),
            Self::NetworkError { message } => format!("Network error: {}", message),
            Self::RateLimited { retry_after } => {
                if let Some(secs) = retry_after {
                    format!("Rate limited, retry after {} seconds", secs)
                } else {
                    "Rate limited, please try again later".to_string()
                }
            }
        }
    }
}

impl std::fmt::Display for SubscriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.user_message())
    }
}

impl std::error::Error for SubscriptionError {}
