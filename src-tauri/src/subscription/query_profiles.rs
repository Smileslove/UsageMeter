use serde::{Deserialize, Serialize};

use crate::models::{SourceCredentialStrategy, SourceQueryProfileId};
use crate::subscription::relay::RelayProvider;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceQuotaProfileCategory {
    GenericBalance,
    NewApiBalance,
    OfficialBalance,
    CodingPlan,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceQuotaExecutorKind {
    GenericBalanceV1Usage,
    NewApiUserSelf,
    RelayProvider,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SourceQuotaProbeKind {
    GenericBalanceV1Usage,
    NewApiUserSelf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceQuotaProfileDescriptor {
    pub profile_id: SourceQueryProfileId,
    pub label_key: String,
    pub category: SourceQuotaProfileCategory,
    pub executor_kind: SourceQuotaExecutorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_kind: Option<SourceQuotaProbeKind>,
    pub default_credential_strategy: SourceCredentialStrategy,
    pub supported_credential_strategies: Vec<SourceCredentialStrategy>,
}

struct QueryProfileDefinition {
    profile_id: SourceQueryProfileId,
    slug: &'static str,
    label_key: &'static str,
    category: SourceQuotaProfileCategory,
    executor_kind: SourceQuotaExecutorKind,
    probe_kind: Option<SourceQuotaProbeKind>,
    default_credential_strategy: SourceCredentialStrategy,
    supported_credential_strategies: &'static [SourceCredentialStrategy],
    relay_providers: &'static [RelayProvider],
    host_patterns: &'static [&'static str],
}

const API_KEY_STRATEGIES: &[SourceCredentialStrategy] = &[
    SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
    SourceCredentialStrategy::ToolLiveApiKey,
    SourceCredentialStrategy::ManualApiKey,
];

const ACCESS_TOKEN_STRATEGIES: &[SourceCredentialStrategy] =
    &[SourceCredentialStrategy::ManualAccessTokenUserId];

const PROFILE_DEFINITIONS: &[QueryProfileDefinition] = &[
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::GenericBalanceV1Usage,
        slug: "generic_balance_v1_usage",
        label_key: "sources.quotaProfileGeneric",
        category: SourceQuotaProfileCategory::GenericBalance,
        executor_kind: SourceQuotaExecutorKind::GenericBalanceV1Usage,
        probe_kind: Some(SourceQuotaProbeKind::GenericBalanceV1Usage),
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[],
        host_patterns: &[],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::NewApiUserSelf,
        slug: "new_api_user_self",
        label_key: "sources.quotaProfileNewApi",
        category: SourceQuotaProfileCategory::NewApiBalance,
        executor_kind: SourceQuotaExecutorKind::NewApiUserSelf,
        probe_kind: Some(SourceQuotaProbeKind::NewApiUserSelf),
        default_credential_strategy: SourceCredentialStrategy::ManualAccessTokenUserId,
        supported_credential_strategies: ACCESS_TOKEN_STRATEGIES,
        relay_providers: &[],
        host_patterns: &[],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::OfficialDeepSeekBalance,
        slug: "deepseek",
        label_key: "sources.quotaProfileDeepSeek",
        category: SourceQuotaProfileCategory::OfficialBalance,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::DeepSeek],
        host_patterns: &["api.deepseek.com"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::OfficialStepFunBalance,
        slug: "stepfun",
        label_key: "sources.quotaProfileStepFun",
        category: SourceQuotaProfileCategory::OfficialBalance,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::StepFun],
        host_patterns: &["api.stepfun.ai", "api.stepfun.com"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::OfficialSiliconFlowBalanceCn,
        slug: "siliconflow_cn",
        label_key: "sources.quotaProfileSiliconFlowCn",
        category: SourceQuotaProfileCategory::OfficialBalance,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::SiliconFlowCn],
        host_patterns: &["api.siliconflow.cn"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::OfficialSiliconFlowBalanceEn,
        slug: "siliconflow_en",
        label_key: "sources.quotaProfileSiliconFlowEn",
        category: SourceQuotaProfileCategory::OfficialBalance,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::SiliconFlowEn],
        host_patterns: &["api.siliconflow.com"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::OfficialOpenRouterBalance,
        slug: "openrouter",
        label_key: "sources.quotaProfileOpenRouter",
        category: SourceQuotaProfileCategory::OfficialBalance,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::OpenRouter],
        host_patterns: &["openrouter.ai"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::OfficialNovitaBalance,
        slug: "novita",
        label_key: "sources.quotaProfileNovita",
        category: SourceQuotaProfileCategory::OfficialBalance,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::Novita],
        host_patterns: &["api.novita.ai"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::KimiCodingPlan,
        slug: "kimi",
        label_key: "sources.quotaProfileKimi",
        category: SourceQuotaProfileCategory::CodingPlan,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::Kimi],
        host_patterns: &["api.kimi.com/coding"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::ZhipuCodingPlan,
        slug: "zhipu",
        label_key: "sources.quotaProfileZhipu",
        category: SourceQuotaProfileCategory::CodingPlan,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::Zhipu, RelayProvider::ZhipuEn],
        host_patterns: &["bigmodel.cn", "api.z.ai"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::MiniMaxCodingPlan,
        slug: "minimax",
        label_key: "sources.quotaProfileMiniMax",
        category: SourceQuotaProfileCategory::CodingPlan,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::MiniMaxCn, RelayProvider::MiniMaxGlobal],
        host_patterns: &["api.minimaxi.com", "api.minimax.io"],
    },
    QueryProfileDefinition {
        profile_id: SourceQueryProfileId::ZenMuxCodingPlan,
        slug: "zenmux",
        label_key: "sources.quotaProfileZenMux",
        category: SourceQuotaProfileCategory::CodingPlan,
        executor_kind: SourceQuotaExecutorKind::RelayProvider,
        probe_kind: None,
        default_credential_strategy: SourceCredentialStrategy::ToolLiveApiKeyThenManualApiKey,
        supported_credential_strategies: API_KEY_STRATEGIES,
        relay_providers: &[RelayProvider::ZenMux],
        host_patterns: &["zenmux."],
    },
];

pub fn list_profile_descriptors() -> Vec<SourceQuotaProfileDescriptor> {
    PROFILE_DEFINITIONS
        .iter()
        .map(|definition| SourceQuotaProfileDescriptor {
            profile_id: definition.profile_id,
            label_key: definition.label_key.to_string(),
            category: definition.category,
            executor_kind: definition.executor_kind,
            probe_kind: definition.probe_kind,
            default_credential_strategy: definition.default_credential_strategy,
            supported_credential_strategies: definition.supported_credential_strategies.to_vec(),
        })
        .collect()
}

fn profile_definition(profile_id: SourceQueryProfileId) -> Option<&'static QueryProfileDefinition> {
    PROFILE_DEFINITIONS
        .iter()
        .find(|definition| definition.profile_id == profile_id)
}

pub fn profile_slug(profile_id: SourceQueryProfileId) -> &'static str {
    profile_definition(profile_id)
        .map(|definition| definition.slug)
        .unwrap_or("unknown")
}

pub fn executor_kind(profile_id: SourceQueryProfileId) -> Option<SourceQuotaExecutorKind> {
    profile_definition(profile_id).map(|definition| definition.executor_kind)
}

pub fn probe_kind(profile_id: SourceQueryProfileId) -> Option<SourceQuotaProbeKind> {
    profile_definition(profile_id).and_then(|definition| definition.probe_kind)
}

pub fn profile_id_for_relay_provider(provider: RelayProvider) -> Option<SourceQueryProfileId> {
    PROFILE_DEFINITIONS
        .iter()
        .find(|definition| definition.relay_providers.contains(&provider))
        .map(|definition| definition.profile_id)
}

pub fn relay_providers_for_profile(profile_id: SourceQueryProfileId) -> &'static [RelayProvider] {
    profile_definition(profile_id)
        .map(|definition| definition.relay_providers)
        .unwrap_or(&[])
}

pub fn detect_builtin_profile(base_url: &str) -> Option<SourceQueryProfileId> {
    let normalized = base_url.to_ascii_lowercase();
    PROFILE_DEFINITIONS
        .iter()
        .filter(|definition| !definition.relay_providers.is_empty())
        .find(|definition| {
            definition
                .host_patterns
                .iter()
                .any(|pattern| normalized.contains(pattern))
        })
        .map(|definition| definition.profile_id)
}

pub fn probe_candidate_profiles() -> &'static [SourceQueryProfileId] {
    const PROBE_CANDIDATES: &[SourceQueryProfileId] = &[
        SourceQueryProfileId::GenericBalanceV1Usage,
        SourceQueryProfileId::NewApiUserSelf,
    ];
    PROBE_CANDIDATES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_expose_expected_defaults() {
        let descriptors = list_profile_descriptors();
        let new_api = descriptors
            .iter()
            .find(|descriptor| descriptor.profile_id == SourceQueryProfileId::NewApiUserSelf)
            .expect("new api descriptor");
        assert_eq!(
            new_api.default_credential_strategy,
            SourceCredentialStrategy::ManualAccessTokenUserId
        );
        assert_eq!(
            new_api.supported_credential_strategies,
            vec![SourceCredentialStrategy::ManualAccessTokenUserId]
        );
        assert_eq!(
            new_api.probe_kind,
            Some(SourceQuotaProbeKind::NewApiUserSelf)
        );
    }

    #[test]
    fn builtin_detection_uses_registry_metadata() {
        assert_eq!(
            detect_builtin_profile("https://openrouter.ai/api/v1"),
            Some(SourceQueryProfileId::OfficialOpenRouterBalance)
        );
        assert_eq!(detect_builtin_profile("https://example.com/v1"), None);
    }
}
