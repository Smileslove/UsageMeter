use crate::copilot::{CopilotAuthStatus, GitHubAccount};
use std::sync::Arc;
use tauri::State;
use tokio::sync::RwLock;

pub struct CopilotAuthState(pub Arc<RwLock<crate::copilot::CopilotAuthManager>>);

#[tauri::command]
pub async fn copilot_list_accounts(
    state: State<'_, CopilotAuthState>,
) -> Result<Vec<GitHubAccount>, String> {
    let mgr = state.0.read().await;
    Ok(mgr.list_accounts().await)
}

#[tauri::command]
pub async fn copilot_get_auth_status(
    state: State<'_, CopilotAuthState>,
) -> Result<CopilotAuthStatus, String> {
    let mgr = state.0.read().await;
    Ok(mgr.get_status().await)
}

#[tauri::command]
pub async fn copilot_is_authenticated(state: State<'_, CopilotAuthState>) -> Result<bool, String> {
    let mgr = state.0.read().await;
    Ok(mgr.is_authenticated().await)
}
