use crate::net::HttpClientFactory;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const GITHUB_COM: &str = "github.com";
const GITHUB_API: &str = "https://api.github.com";
const EDITOR_VERSION: &str = "vscode/1.110.1";
const EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.38.2";
const COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.38.2";
const GITHUB_API_VERSION: &str = "2025-10-01";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccount {
    pub id: String,
    pub login: String,
    pub avatar_url: Option<String>,
    pub authenticated_at: i64,
    pub github_domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotUsageResponse {
    pub copilot_plan: String,
    pub quota_reset_date: String,
    pub quota_snapshots: QuotaSnapshots,
    #[serde(default)]
    pub endpoints: Option<CopilotEndpoints>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSnapshots {
    pub chat: QuotaDetail,
    pub completions: QuotaDetail,
    pub premium_interactions: QuotaDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaDetail {
    pub entitlement: i64,
    pub remaining: i64,
    pub percent_remaining: f64,
    pub unlimited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopilotEndpoints {
    pub api: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopilotAuthStatus {
    pub accounts: Vec<GitHubAccount>,
    pub default_account_id: Option<String>,
    pub authenticated: bool,
    pub username: Option<String>,
    pub migration_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotInstallEntry {
    user: Option<String>,
    oauth_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotErrorResponse {
    message: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CopilotAuthError {
    #[error("copilot.no_local_credential")]
    NoLocalCredential,
    #[error("copilot.token_invalid")]
    GitHubTokenInvalid,
    #[error("copilot.no_subscription")]
    NoCopilotSubscription,
    #[error("copilot.network_error:{0}")]
    NetworkError(String),
    #[error("copilot.parse_error:{0}")]
    ParseError(String),
    #[error("copilot.io_error:{0}")]
    IoError(String),
}

pub struct CopilotAuthManager {
    config_dir: PathBuf,
}

impl CopilotAuthManager {
    pub fn new(_data_dir: PathBuf) -> Self {
        Self {
            config_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
                .join("github-copilot"),
        }
    }

    fn http_client(&self) -> Client {
        HttpClientFactory::global().standard()
    }

    pub async fn list_accounts(&self) -> Vec<GitHubAccount> {
        self.load_local_accounts()
            .unwrap_or_default()
            .into_iter()
            .map(|account| account.public)
            .collect()
    }

    pub async fn get_status(&self) -> CopilotAuthStatus {
        let accounts = self.list_accounts().await;
        let username = accounts.first().map(|account| account.login.clone());
        CopilotAuthStatus {
            default_account_id: accounts.first().map(|account| account.id.clone()),
            authenticated: !accounts.is_empty(),
            accounts,
            username,
            migration_error: None,
        }
    }

    pub async fn is_authenticated(&self) -> bool {
        self.load_local_accounts()
            .map(|accounts| !accounts.is_empty())
            .unwrap_or(false)
    }

    pub async fn fetch_usage(&self) -> Result<CopilotUsageResponse, CopilotAuthError> {
        let accounts = self.load_local_accounts()?;
        let account = accounts
            .into_iter()
            .next()
            .ok_or(CopilotAuthError::NoLocalCredential)?;
        self.fetch_usage_with_token(&account.token, &account.github_domain)
            .await
    }

    fn load_local_accounts(&self) -> Result<Vec<LocalCopilotAccount>, CopilotAuthError> {
        let mut accounts = Vec::new();
        let mut last_error: Option<CopilotAuthError> = None;
        for path in self.credential_file_candidates() {
            if !path.exists() {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(err) => {
                    last_error = Some(CopilotAuthError::IoError(err.to_string()));
                    continue;
                }
            };
            let parsed =
                match serde_json::from_str::<HashMap<String, CopilotInstallEntry>>(&content) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        last_error = Some(CopilotAuthError::ParseError(err.to_string()));
                        continue;
                    }
                };

            for (raw_key, entry) in parsed {
                let Some(token) = entry.oauth_token.filter(|value| !value.trim().is_empty()) else {
                    continue;
                };
                let github_domain = raw_key
                    .split(':')
                    .next()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(GITHUB_COM)
                    .to_string();
                let login = entry
                    .user
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| github_domain.clone());

                accounts.push(LocalCopilotAccount {
                    token,
                    github_domain: github_domain.clone(),
                    public: GitHubAccount {
                        id: raw_key,
                        login,
                        avatar_url: None,
                        authenticated_at: 0,
                        github_domain,
                    },
                });
            }
        }

        if accounts.is_empty() {
            if let Some(err) = last_error {
                return Err(err);
            }
        }

        accounts.sort_by(|a, b| a.public.id.cmp(&b.public.id));
        accounts.dedup_by(|a, b| a.public.id == b.public.id);
        accounts.sort_by(|a, b| a.public.login.cmp(&b.public.login));
        Ok(accounts)
    }

    fn credential_file_candidates(&self) -> Vec<PathBuf> {
        vec![
            self.config_dir.join("apps.json"),
            self.config_dir.join("hosts.json"),
        ]
    }

    async fn fetch_usage_with_token(
        &self,
        token: &str,
        domain: &str,
    ) -> Result<CopilotUsageResponse, CopilotAuthError> {
        let url = format!("{}/copilot_internal/user", api_base_url(domain));
        let response = self
            .http_client()
            .get(url)
            .header("Authorization", format!("token {token}"))
            .header("Content-Type", "application/json")
            .header("Editor-Version", EDITOR_VERSION)
            .header("Editor-Plugin-Version", EDITOR_PLUGIN_VERSION)
            .header("User-Agent", COPILOT_USER_AGENT)
            .header("x-github-api-version", GITHUB_API_VERSION)
            .send()
            .await
            .map_err(|err| CopilotAuthError::NetworkError(err.to_string()))?;

        let status = response.status().as_u16();
        match status {
            200 => response
                .json::<CopilotUsageResponse>()
                .await
                .map_err(|err| CopilotAuthError::ParseError(err.to_string())),
            401 => Err(CopilotAuthError::GitHubTokenInvalid),
            403 => {
                let body = response.text().await.unwrap_or_default();
                if is_no_copilot_subscription_response(&body) {
                    Err(CopilotAuthError::NoCopilotSubscription)
                } else {
                    Err(CopilotAuthError::NetworkError(format!("HTTP 403: {body}")))
                }
            }
            429 => Err(CopilotAuthError::NetworkError("rate_limited".to_string())),
            status => Err(CopilotAuthError::NetworkError(format!("HTTP {status}"))),
        }
    }
}

struct LocalCopilotAccount {
    token: String,
    github_domain: String,
    public: GitHubAccount,
}

fn api_base_url(domain: &str) -> String {
    if domain == GITHUB_COM {
        GITHUB_API.to_string()
    } else {
        format!("https://{domain}/api/v3")
    }
}

fn is_no_copilot_subscription_response(body: &str) -> bool {
    let normalized = serde_json::from_str::<CopilotErrorResponse>(body)
        .ok()
        .and_then(|payload| payload.message)
        .unwrap_or_else(|| body.to_string())
        .to_lowercase();

    normalized.contains("no copilot")
        || normalized.contains("not subscribed")
        || normalized.contains("no subscription")
        || normalized.contains("copilot business is not enabled")
}
