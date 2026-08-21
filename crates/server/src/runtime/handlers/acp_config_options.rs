use super::super::*;

use std::collections::BTreeSet;

use crate::AcpErrorCode;
use crate::AcpSetConfigOptionParams;
use crate::runtime::session_actor::snapshots::HookContextSnapshot;
use crate::session_context::SessionRuntimeContext;
use devo_core::SessionConfig;
use devo_core::TurnConfig;
use devo_protocol::AcpSessionConfigOption;
use devo_protocol::AcpSessionConfigOptionCategory;
use devo_protocol::AcpSessionConfigOptionCategoryKnown;
use devo_protocol::AcpSessionConfigSelectOption;
use devo_protocol::AcpSessionConfigSelectOptions;
use devo_protocol::Model;
use devo_protocol::PermissionPreset;
use devo_protocol::SessionId;

const ACP_MODE_CONFIG_ID: &str = "mode";
pub(crate) const ACP_MODEL_CONFIG_ID: &str = "model";
pub(crate) const ACP_REASONING_EFFORT_CONFIG_ID: &str = "thought_level";
pub(crate) const ACP_SANDBOX_PROFILE_CONFIG_ID: &str = "sandbox_profile";

impl ServerRuntime {
    pub(super) async fn acp_session_config_options(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<AcpSessionConfigOption>, String> {
        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Err("session does not exist".to_string());
        };
        let snapshot: HookContextSnapshot = session_arc
            .hook_context_snapshot()
            .await
            .ok_or_else(|| "session actor unavailable".to_string())?;
        Ok(acp_config_options_for_session(
            &snapshot.runtime_context,
            &snapshot.summary,
            &snapshot.config,
        ))
    }

    pub(crate) fn acp_model_config_options_for_context(
        &self,
        runtime_context: &SessionRuntimeContext,
    ) -> Vec<AcpSessionConfigOption> {
        let turn_config = runtime_context.resolve_turn_config(None, None);
        acp_model_and_reasoning_options_for_context(runtime_context, &turn_config)
    }

    pub(super) async fn set_acp_session_config_option(
        &self,
        params: AcpSetConfigOptionParams,
    ) -> Result<Vec<AcpSessionConfigOption>, (AcpErrorCode, String)> {
        let Some(value) = params.value.string_value().map(str::to_owned) else {
            return Err((
                AcpErrorCode::InvalidParams,
                "boolean session config options are not supported".to_string(),
            ));
        };
        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
            return Err((
                AcpErrorCode::ServerError,
                "session does not exist".to_string(),
            ));
        };
        let _state_change_guard = session_arc.lock_state_change().await;

        let snapshot: HookContextSnapshot =
            session_arc.hook_context_snapshot().await.ok_or_else(|| {
                (
                    AcpErrorCode::ServerError,
                    "session actor unavailable".to_string(),
                )
            })?;

        let (updated_session, config_options) = match params.config_id.as_str() {
            ACP_MODEL_CONFIG_ID => {
                let model_option = acp_model_config_option_for_session(
                    &snapshot.runtime_context,
                    &snapshot.summary,
                    &snapshot.config,
                );
                let value_is_allowed = match &model_option {
                    AcpSessionConfigOption::Select { options, .. } => {
                        select_options_contain_value(options, &value)
                    }
                    AcpSessionConfigOption::Boolean { .. } => false,
                };
                if !value_is_allowed {
                    return Err((
                        AcpErrorCode::InvalidParams,
                        format!(
                            "invalid value '{}' for session config option '{}'",
                            value, params.config_id
                        ),
                    ));
                }

                let mut turn_config = snapshot.runtime_context.resolve_turn_config(
                    Some(value.as_str()),
                    snapshot.summary.reasoning_effort_selection.clone(),
                );
                turn_config.reasoning_effort_selection = current_reasoning_effort_value(
                    &turn_config.model,
                    snapshot.summary.reasoning_effort_selection.as_deref(),
                );

                let updated = session_arc
                    .update_session_metadata(
                        Some(turn_config.model.slug.clone()),
                        turn_config.model_binding_id.clone(),
                        turn_config.reasoning_effort_selection.clone(),
                        None,
                    )
                    .await
                    .ok_or_else(|| {
                        (
                            AcpErrorCode::ServerError,
                            "failed to update session".to_string(),
                        )
                    })?;

                let updated_snapshot =
                    session_arc.hook_context_snapshot().await.ok_or_else(|| {
                        (
                            AcpErrorCode::ServerError,
                            "session actor unavailable".to_string(),
                        )
                    })?;
                if let Some(record) = updated_snapshot.record.as_ref()
                    && let Err(error) = self.rollout_store.append_session_meta(record)
                {
                    return Err((AcpErrorCode::ServerError, error.to_string()));
                }

                (
                    Some(updated),
                    acp_config_options_for_session(
                        &updated_snapshot.runtime_context,
                        &updated_snapshot.summary,
                        &updated_snapshot.config,
                    ),
                )
            }
            ACP_REASONING_EFFORT_CONFIG_ID => {
                let Some(reasoning_effort_option) = acp_reasoning_effort_config_option_for_session(
                    &snapshot.runtime_context,
                    &snapshot.summary,
                ) else {
                    return Err((
                        AcpErrorCode::InvalidParams,
                        format!("unknown session config option '{}'", params.config_id),
                    ));
                };
                let value_is_allowed = match &reasoning_effort_option {
                    AcpSessionConfigOption::Select { options, .. } => {
                        select_options_contain_value(options, &value)
                    }
                    AcpSessionConfigOption::Boolean { .. } => false,
                };
                if !value_is_allowed {
                    return Err((
                        AcpErrorCode::InvalidParams,
                        format!(
                            "invalid value '{}' for session config option '{}'",
                            value, params.config_id
                        ),
                    ));
                }

                let mut turn_config = snapshot.runtime_context.resolve_turn_config(
                    session_model_selection(&snapshot.summary),
                    Some(value.clone()),
                );
                turn_config.reasoning_effort_selection = Some(value.clone());

                let updated = session_arc
                    .update_session_metadata(
                        Some(turn_config.model.slug.clone()),
                        turn_config.model_binding_id.clone(),
                        turn_config.reasoning_effort_selection.clone(),
                        None,
                    )
                    .await
                    .ok_or_else(|| {
                        (
                            AcpErrorCode::ServerError,
                            "failed to update session".to_string(),
                        )
                    })?;

                let updated_snapshot =
                    session_arc.hook_context_snapshot().await.ok_or_else(|| {
                        (
                            AcpErrorCode::ServerError,
                            "session actor unavailable".to_string(),
                        )
                    })?;
                if let Some(record) = updated_snapshot.record.as_ref()
                    && let Err(error) = self.rollout_store.append_session_meta(record)
                {
                    return Err((AcpErrorCode::ServerError, error.to_string()));
                }

                (
                    Some(updated),
                    acp_config_options_for_session(
                        &updated_snapshot.runtime_context,
                        &updated_snapshot.summary,
                        &updated_snapshot.config,
                    ),
                )
            }
            ACP_MODE_CONFIG_ID => {
                let Some(preset) = permission_preset_from_value(&value) else {
                    return Err((
                        AcpErrorCode::InvalidParams,
                        format!(
                            "invalid value '{}' for session config option '{}'",
                            value, params.config_id
                        ),
                    ));
                };
                let profile = safety_profile_from_protocol(
                    preset,
                    snapshot.summary.cwd.clone(),
                    snapshot.summary.additional_directories.clone(),
                );

                if !session_arc.apply_permission_profile(profile.clone()).await {
                    return Err((
                        AcpErrorCode::ServerError,
                        "failed to apply permission profile".to_string(),
                    ));
                }

                let updated_snapshot =
                    session_arc.hook_context_snapshot().await.ok_or_else(|| {
                        (
                            AcpErrorCode::ServerError,
                            "session actor unavailable".to_string(),
                        )
                    })?;
                (
                    None,
                    acp_config_options_for_session(
                        &updated_snapshot.runtime_context,
                        &updated_snapshot.summary,
                        &updated_snapshot.config,
                    ),
                )
            }
            ACP_SANDBOX_PROFILE_CONFIG_ID => {
                match session_arc.apply_sandbox_profile(value.clone()).await {
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        return Err((
                            AcpErrorCode::InvalidParams,
                            format!(
                                "invalid value '{}' for session config option '{}': {error}",
                                value, params.config_id
                            ),
                        ));
                    }
                    None => {
                        return Err((
                            AcpErrorCode::ServerError,
                            "failed to apply sandbox profile".to_string(),
                        ));
                    }
                }

                let updated_snapshot =
                    session_arc.hook_context_snapshot().await.ok_or_else(|| {
                        (
                            AcpErrorCode::ServerError,
                            "session actor unavailable".to_string(),
                        )
                    })?;
                (
                    None,
                    acp_config_options_for_session(
                        &updated_snapshot.runtime_context,
                        &updated_snapshot.summary,
                        &updated_snapshot.config,
                    ),
                )
            }
            _ => {
                return Err((
                    AcpErrorCode::InvalidParams,
                    format!("unknown session config option '{}'", params.config_id),
                ));
            }
        };

        if let Some(updated_session) = updated_session
            && !updated_session.ephemeral
            && let Err(error) = self.deps.db.upsert_session(&updated_session, None)
        {
            tracing::warn!(
                session_id = %params.session_id,
                %error,
                "failed to persist ACP session config option"
            );
        }

        Ok(config_options)
    }
}

fn acp_config_options_for_session(
    runtime_context: &SessionRuntimeContext,
    summary: &SessionMetadata,
    config: &SessionConfig,
) -> Vec<AcpSessionConfigOption> {
    let mut options = acp_model_and_reasoning_options_for_session(runtime_context, summary);
    options.push(acp_mode_config_option_for_session(config));
    options
}

fn acp_model_and_reasoning_options_for_session(
    runtime_context: &SessionRuntimeContext,
    summary: &SessionMetadata,
) -> Vec<AcpSessionConfigOption> {
    let turn_config = runtime_context.resolve_turn_config(
        session_model_selection(summary),
        summary.reasoning_effort_selection.clone(),
    );
    acp_model_and_reasoning_options_for_context(runtime_context, &turn_config)
}

fn acp_model_and_reasoning_options_for_context(
    runtime_context: &SessionRuntimeContext,
    turn_config: &TurnConfig,
) -> Vec<AcpSessionConfigOption> {
    let mut options = vec![acp_model_config_option_for_turn_config(
        runtime_context,
        turn_config,
    )];
    if let Some(reasoning_effort_option) =
        acp_reasoning_effort_config_option_for_turn_config(turn_config)
    {
        options.push(reasoning_effort_option);
    }
    options
}

fn acp_mode_config_option_for_session(config: &SessionConfig) -> AcpSessionConfigOption {
    let current_value = permission_preset_value(permission_preset_from_safety(
        config.permission_profile.preset,
    ))
    .to_string();
    AcpSessionConfigOption::Select {
        id: ACP_MODE_CONFIG_ID.to_string(),
        name: "Session Mode".to_string(),
        description: Some("Controls how Devo requests permission".to_string()),
        category: Some(AcpSessionConfigOptionCategory::Known(
            AcpSessionConfigOptionCategoryKnown::Mode,
        )),
        current_value,
        options: AcpSessionConfigSelectOptions::Ungrouped(
            [
                (
                    PermissionPreset::Default,
                    "Ask for approval",
                    "Workspace sandbox; read/edit/run in workspace. You approve sensitive tools.",
                ),
                (
                    PermissionPreset::AutoReview,
                    "Approve for me",
                    "Same sandbox as Ask for approval. An AI reviewer may approve low-risk tools; uncertain ones still ask you.",
                ),
                (
                    PermissionPreset::FullAccess,
                    "Full-Access",
                    "No OS sandbox and no approval prompts; use with caution.",
                ),
            ]
            .into_iter()
            .map(|(preset, name, description)| AcpSessionConfigSelectOption {
                value: permission_preset_value(preset).to_string(),
                name: name.to_string(),
                description: Some(description.to_string()),
                meta: None,
            })
            .collect(),
        ),
        meta: None,
    }
}

fn acp_model_config_option_for_session(
    runtime_context: &SessionRuntimeContext,
    summary: &SessionMetadata,
    _config: &SessionConfig,
) -> AcpSessionConfigOption {
    let turn_config = runtime_context.resolve_turn_config(
        session_model_selection(summary),
        summary.reasoning_effort_selection.clone(),
    );
    acp_model_config_option_for_turn_config(runtime_context, &turn_config)
}

fn acp_model_config_option_for_turn_config(
    runtime_context: &SessionRuntimeContext,
    turn_config: &TurnConfig,
) -> AcpSessionConfigOption {
    let current_value = turn_config
        .model_binding_id
        .clone()
        .unwrap_or_else(|| turn_config.model.slug.clone());
    let config = runtime_context
        .config_store
        .lock()
        .expect("app config store mutex should not be poisoned")
        .effective_config()
        .clone();

    let mut options = Vec::new();
    let mut seen_values = BTreeSet::new();
    for (binding_id, binding) in &config.provider.model_bindings {
        if !binding.enabled || !seen_values.insert(binding_id.clone()) {
            continue;
        }
        let model_display_name = runtime_context
            .model_catalog
            .get(&binding.model_slug)
            .map(|model| model.display_name.as_str())
            .and_then(non_empty_str);
        let name = binding
            .display_name
            .as_deref()
            .and_then(non_empty_str)
            .or(model_display_name)
            .unwrap_or(binding.model_slug.as_str())
            .to_string();
        let provider_name = config
            .provider
            .providers
            .get(&binding.provider)
            .map(|provider| provider.name.as_str())
            .and_then(non_empty_str)
            .unwrap_or(binding.provider.as_str());
        options.push(AcpSessionConfigSelectOption {
            value: binding_id.clone(),
            name,
            description: Some(format!(
                "{provider_name}: {} via {}",
                binding.request_model, binding.invocation_method
            )),
            meta: None,
        });
    }

    if !seen_values.contains(&current_value) {
        options.insert(
            0,
            AcpSessionConfigSelectOption {
                value: current_value.clone(),
                name: current_value.clone(),
                description: None,
                meta: None,
            },
        );
    }

    AcpSessionConfigOption::Select {
        id: ACP_MODEL_CONFIG_ID.to_string(),
        name: "Model".to_string(),
        description: Some("Controls the model used for this session".to_string()),
        category: Some(AcpSessionConfigOptionCategory::Known(
            AcpSessionConfigOptionCategoryKnown::Model,
        )),
        current_value,
        options: AcpSessionConfigSelectOptions::Ungrouped(options),
        meta: None,
    }
}

fn acp_reasoning_effort_config_option_for_session(
    runtime_context: &SessionRuntimeContext,
    summary: &SessionMetadata,
) -> Option<AcpSessionConfigOption> {
    let turn_config = runtime_context.resolve_turn_config(
        session_model_selection(summary),
        summary.reasoning_effort_selection.clone(),
    );
    acp_reasoning_effort_config_option_for_turn_config(&turn_config)
}

fn acp_reasoning_effort_config_option_for_turn_config(
    turn_config: &TurnConfig,
) -> Option<AcpSessionConfigOption> {
    let current_value = current_reasoning_effort_value(
        &turn_config.model,
        turn_config.reasoning_effort_selection.as_deref(),
    )?;
    let mut seen_values = BTreeSet::new();
    let mut options = Vec::new();
    for preset in turn_config.model.effective_reasoning_capability().options() {
        if !seen_values.insert(preset.value.clone()) {
            continue;
        }
        options.push(AcpSessionConfigSelectOption {
            value: preset.value,
            name: preset.label,
            description: Some(preset.description),
            meta: None,
        });
    }

    if options.is_empty() {
        return None;
    }

    if !seen_values.contains(&current_value) {
        options.insert(
            0,
            AcpSessionConfigSelectOption {
                value: current_value.clone(),
                name: current_value.clone(),
                description: None,
                meta: None,
            },
        );
    }

    Some(AcpSessionConfigOption::Select {
        id: ACP_REASONING_EFFORT_CONFIG_ID.to_string(),
        name: "Reasoning Effort".to_string(),
        description: Some("Controls the model reasoning effort used for this session".to_string()),
        category: Some(AcpSessionConfigOptionCategory::Known(
            AcpSessionConfigOptionCategoryKnown::ThoughtLevel,
        )),
        current_value,
        options: AcpSessionConfigSelectOptions::Ungrouped(options),
        meta: None,
    })
}

fn current_reasoning_effort_value(model: &Model, selection: Option<&str>) -> Option<String> {
    let option_values = model
        .effective_reasoning_capability()
        .options()
        .into_iter()
        .map(|option| option.value)
        .collect::<Vec<_>>();
    if option_values.is_empty() {
        return None;
    }
    model
        .normalize_reasoning_effort_selection(selection)
        .filter(|value| option_values.contains(value))
        .or_else(|| {
            model
                .default_reasoning_effort_selection()
                .filter(|value| option_values.contains(value))
        })
        .or_else(|| option_values.first().cloned())
}

pub(crate) fn select_options_contain_value(
    options: &AcpSessionConfigSelectOptions,
    value: &str,
) -> bool {
    match options {
        AcpSessionConfigSelectOptions::Ungrouped(options) => {
            options.iter().any(|option| option.value == value)
        }
        AcpSessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter())
            .any(|option| option.value == value),
    }
}

fn non_empty_str(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn permission_preset_value(preset: PermissionPreset) -> &'static str {
    match preset {
        PermissionPreset::Default => "default",
        PermissionPreset::AutoReview => "auto-review",
        PermissionPreset::FullAccess => "full-access",
    }
}

fn permission_preset_from_value(value: &str) -> Option<PermissionPreset> {
    match value {
        "default" => Some(PermissionPreset::Default),
        "read-only" => {
            tracing::warn!(
                "permission_preset \"read-only\" is deprecated and maps to Default \
                 (shell allowed); set sandbox_profile = \"read-only\" for a \
                 read-only OS sandbox instead"
            );
            Some(PermissionPreset::Default)
        }
        "auto-review" => Some(PermissionPreset::AutoReview),
        "full-access" => Some(PermissionPreset::FullAccess),
        _ => None,
    }
}

fn permission_preset_from_safety(preset: devo_safety::PermissionPreset) -> PermissionPreset {
    match preset {
        devo_safety::PermissionPreset::Default => PermissionPreset::Default,
        devo_safety::PermissionPreset::AutoReview => PermissionPreset::AutoReview,
        devo_safety::PermissionPreset::FullAccess => PermissionPreset::FullAccess,
    }
}
