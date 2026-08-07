use super::types::MergedRequestFact;
use crate::models::SourceFilter;
use crate::session::SessionMeta;

#[derive(Debug, Clone)]
pub(super) struct ProjectDescriptor {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) identity: String,
    pub(super) path: Option<String>,
}

pub(super) fn metadata_only_sessions_allowed(source_filter: &SourceFilter) -> bool {
    matches!(
        source_filter,
        SourceFilter::All | SourceFilter::Unknown { .. }
    )
}

pub(super) fn session_project_identity(
    project_name: Option<&str>,
    cwd: Option<&str>,
) -> &'static str {
    if project_name.is_some_and(|value| !value.trim().is_empty())
        || cwd.is_some_and(|value| !value.trim().is_empty())
    {
        "project"
    } else {
        "unknown"
    }
}

pub(super) fn project_descriptor_for_session(meta: &SessionMeta) -> ProjectDescriptor {
    let project_name = non_empty_owned(meta.project_name.as_deref());
    let project_path = non_empty_owned(meta.cwd.as_deref());

    if let Some(path) = project_path {
        return ProjectDescriptor {
            key: path.clone(),
            name: project_name.unwrap_or_else(|| path.clone()),
            identity: "project".to_string(),
            path: Some(path),
        };
    }

    if let Some(name) = project_name {
        return ProjectDescriptor {
            key: format!("name::{name}"),
            name,
            identity: "project".to_string(),
            path: None,
        };
    }

    ProjectDescriptor {
        key: format!("unknown::{}", meta.session_id),
        name: meta.session_id.clone(),
        identity: "unknown".to_string(),
        path: None,
    }
}

pub(super) fn project_descriptor_for_fact(fact: &MergedRequestFact) -> ProjectDescriptor {
    let project_name = non_empty_owned(fact.project_name.as_deref());
    let project_path = non_empty_owned(fact.project_path.as_deref());

    if let Some(path) = project_path {
        return ProjectDescriptor {
            key: path.clone(),
            name: project_name.unwrap_or_else(|| path.clone()),
            identity: "project".to_string(),
            path: Some(path),
        };
    }

    if let Some(name) = project_name {
        return ProjectDescriptor {
            key: format!("name::{name}"),
            name,
            identity: "project".to_string(),
            path: None,
        };
    }

    let key = if fact.session_id.trim().is_empty() {
        format!("unknown::fact::{}", fact.canonical_request_key)
    } else {
        format!("unknown::{}", fact.session_id)
    };
    ProjectDescriptor {
        key,
        name: fact.session_id.clone(),
        identity: "unknown".to_string(),
        path: None,
    }
}

fn non_empty_owned(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_identity_ignores_blank_metadata() {
        assert_eq!(session_project_identity(Some("  "), Some("")), "unknown");
        assert_eq!(session_project_identity(None, Some("/repo")), "project");
    }

    #[test]
    fn session_descriptor_prefers_path_as_stable_key() {
        let meta = SessionMeta {
            session_id: "session-1".to_string(),
            project_name: Some("UsageMeter".to_string()),
            cwd: Some("/workspace/UsageMeter".to_string()),
            ..Default::default()
        };
        let descriptor = project_descriptor_for_session(&meta);
        assert_eq!(descriptor.key, "/workspace/UsageMeter");
        assert_eq!(descriptor.name, "UsageMeter");
    }
}
