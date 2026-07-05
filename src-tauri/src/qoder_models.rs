const QODER_MODEL_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("auto", "Auto"),
    ("dfmodel", "DeepSeek-V4-Flash"),
    ("dmodel", "DeepSeek-V4-Pro"),
    ("gm51model", "GLM-5.2"),
    ("kmodel", "Kimi-K2.7-Code"),
    ("mmodel", "MiniMax-M2.7"),
    ("q36fmodel", "Qwen3.6-Flash"),
    ("qmodel", "Qwen3.7-Plus"),
    ("qmodel_latest", "Qwen3.7-Max"),
    ("qwork-advanced", "Advanced"),
    ("qwork-auto", "Standard"),
    ("qwork-ultimate", "Premium"),
];

pub(crate) fn qoder_model_display_name(model_id: &str) -> Option<&'static str> {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return None;
    }

    QODER_MODEL_DISPLAY_NAMES
        .iter()
        .find_map(|(id, display)| trimmed.eq_ignore_ascii_case(id).then_some(*display))
}

pub(crate) fn normalize_qoder_model_name(model: &str) -> String {
    qoder_model_display_name(model)
        .unwrap_or_else(|| model.trim())
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_qoder_internal_model_ids_to_display_names() {
        assert_eq!(qoder_model_display_name("gm51model"), Some("GLM-5.2"));
        assert_eq!(
            qoder_model_display_name("qmodel_latest"),
            Some("Qwen3.7-Max")
        );
        assert_eq!(qoder_model_display_name("auto"), Some("Auto"));
        assert_eq!(qoder_model_display_name("unknown-model"), None);
    }
}
