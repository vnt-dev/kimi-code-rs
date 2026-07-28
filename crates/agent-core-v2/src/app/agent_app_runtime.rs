//! Application composition used by hosts that run the complete Agent runtime.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use crate::{
    _base::{
        di::service_collection::ServiceCollection,
        log::{LOG_OPTIONS_ID, register_log_service, resolve_logging_config},
    },
    app::bootstrap::{BootstrapInput, bootstrap_with_extra, resolve_bootstrap_options},
};

pub fn bootstrap_agent_app(
    input: BootstrapInput,
) -> Result<crate::_base::di::scope::Scope, String> {
    let options = resolve_bootstrap_options(input.clone()).map_err(|error| error.to_string())?;
    register_agent_app_contributions()?;
    register_agent_app_services();

    let logging = resolve_logging_config(&options.home_dir, &HashMap::new());
    let mut extra = ServiceCollection::new();
    extra.set_instance(LOG_OPTIONS_ID, Arc::new(logging));
    bootstrap_with_extra(input, extra)
        .map(|result| result.app)
        .map_err(|error| error.to_string())
}

pub fn register_agent_app_contributions() -> Result<(), String> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTERED
        .get_or_init(register_agent_app_contributions_once)
        .clone()
}

fn register_agent_app_contributions_once() -> Result<(), String> {
    use crate::{
        agent::{
            external_hooks::register_hooks_config_section,
            loop_::register_loop_control_config_section,
            media::register_image_config_section,
            permission_mode::register_default_permission_mode_config_section,
            permission_rules::register_permission_config_section,
            plan::{
                register_default_plan_mode_config_section, register_enter_plan_mode_tool,
                register_exit_plan_mode_tool, register_plan_agent_profile,
            },
            question_tools::register_ask_user_question_tool,
            task::register_task_config_sections,
            tool_policy::register_tools_config_section,
        },
        app::{
            agent_file_catalog::register_agent_file_catalog_config_sections,
            auth::{register_services_config_section, register_web_search_tool},
            cron::register_cron_config_section,
            flag::register_experimental_config_section,
            skill_catalog::register_skill_catalog_config_sections,
        },
        kosong::{
            model::{
                register_model_catalog_config_section, register_models_config_section,
                thinking::register_thinking_config_section,
            },
            provider::{
                bases::{
                    anthropic::anthropic_contrib::ensure_anthropic_base_registered,
                    google_genai::google_genai_contrib::ensure_google_gen_ai_base_registered,
                    openai::{
                        openai_legacy_contrib::ensure_openai_legacy_base_registered,
                        openai_responses_contrib::ensure_openai_responses_base_registered,
                    },
                },
                providers::ensure_provider_definitions_registered,
                register_provider_config_section,
            },
        },
        session::{
            agent_lifecycle::register_builtin_agent_lifecycle_profiles,
            subagent::register_subagent_config_section,
        },
    };

    register_provider_config_section();
    register_models_config_section();
    register_model_catalog_config_section();
    register_thinking_config_section();
    register_services_config_section();
    register_experimental_config_section();
    register_tools_config_section();
    register_loop_control_config_section();
    register_agent_file_catalog_config_sections();
    register_skill_catalog_config_sections();
    register_task_config_sections();
    register_permission_config_section();
    register_default_permission_mode_config_section();
    register_default_plan_mode_config_section();
    register_image_config_section();
    register_hooks_config_section();
    register_cron_config_section();
    register_subagent_config_section();
    register_builtin_agent_lifecycle_profiles();
    register_plan_agent_profile();

    ensure_provider_definitions_registered().map_err(|error| error.to_string())?;
    ensure_openai_legacy_base_registered().map_err(|error| error.to_string())?;
    ensure_openai_responses_base_registered().map_err(|error| error.to_string())?;
    ensure_anthropic_base_registered().map_err(|error| error.to_string())?;
    ensure_google_gen_ai_base_registered().map_err(|error| error.to_string())?;

    crate::agent::tool_select::register_select_tools_tool();
    crate::app::web::register_fetch_url_tool();
    register_web_search_tool();
    crate::agent::task::tools::register_task_list_tool();
    crate::agent::task::tools::register_task_output_tool();
    crate::agent::task::tools::register_task_stop_tool();
    register_enter_plan_mode_tool();
    register_exit_plan_mode_tool();
    register_ask_user_question_tool();
    crate::os::backends::node_local::tools::register_node_local_tools();
    crate::session::subagent::register_agent_tool();
    Ok(())
}

pub fn register_agent_app_services() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(register_agent_app_services_once);
}

fn register_agent_app_services_once() {
    use crate::{
        agent::{
            activity_view::register_agent_activity_view_service,
            blob::register_agent_blob_service,
            context_injector::register_agent_context_injector_service,
            context_memory::register_agent_context_memory_service,
            context_projector::register_agent_context_projector_service,
            context_size::register_agent_context_size_service,
            external_hooks::register_agent_external_hooks_service,
            fault_injection::register_fault_injection_service,
            full_compaction::register_agent_full_compaction_service,
            goal::{register_agent_goal_service, register_goal_deadline_scheduler_service},
            llm_requester::register_agent_llm_requester_service,
            loop_::{register_agent_loop_continuation_service, register_agent_loop_service},
            mcp::register_agent_mcp_service,
            media::{register_agent_media_tools_registrar, register_image_config_bridge},
            permission_gate::register_agent_permission_gate,
            permission_mode::register_agent_permission_mode_service,
            permission_policy::register_agent_permission_policy_service,
            permission_rules::register_agent_permission_rules_service,
            plan::register_agent_plan_service,
            plugin::register_agent_plugin_service,
            profile::register_agent_profile_service,
            prompt::register_agent_prompt_service,
            rpc::register_agent_rpc_service,
            shell_command::register_agent_shell_command_service,
            skill::register_agent_skill_service,
            step_retry::register_agent_step_retry_service,
            swarm::register_agent_swarm_service,
            system_reminder::register_agent_system_reminder_service,
            task::register_agent_task_service,
            tool_dedupe::register_agent_tool_dedupe_service,
            tool_executor::register_agent_tool_executor_service,
            tool_policy::register_agent_tool_policy_service,
            tool_registry::{
                register_agent_builtin_tools_registrar, register_agent_tool_registry_service,
            },
            tool_result_truncation::register_tool_result_truncation_service,
            tool_select::{
                register_agent_tool_select_announcements_service,
                register_agent_tool_select_service,
            },
            usage::register_usage_service,
            user_tool::register_agent_user_tool_service,
        },
        app::{
            agent_file_catalog::{
                register_agent_catalog_runtime_options, register_user_file_agent_source,
            },
            agent_profile_catalog::register_agent_profile_catalog_service,
            auth::{
                register_oauth_service, register_oauth_toolkit_service,
                register_web_search_provider_service,
            },
            config::{register_config_registry, register_config_service},
            cron::register_cron_task_persistence_service,
            event::{register_event_bus_service, register_event_service},
            external_hooks_runner::register_external_hooks_runner_service,
            file::register_file_service,
            flag::{register_flag_registry_service, register_flag_service},
            message_legacy::register_message_legacy_service,
            plugin::register_plugin_service,
            session_index::register_session_index_service,
            session_lifecycle::register_session_lifecycle_service,
            skill_catalog::{
                register_builtin_skill_source, register_skill_catalog_runtime_options,
                register_user_file_skill_source,
            },
            telemetry::{register_agent_telemetry_context_service, register_telemetry_service},
            web::register_web_fetch_service,
            workspace_registry::{
                register_workspace_persistence, register_workspace_query_service,
                register_workspace_registry_service,
            },
        },
        kosong::{
            model::{
                register_host_request_headers, register_model_catalog, register_model_service,
                register_provider_discovery_service,
            },
            provider::{register_protocol_adapter_registry, register_provider_service},
        },
        os::backends::node_local::{
            host_environment_service::register_local_host_environment_service,
            host_fs_service::register_local_host_file_system_service,
            host_process_service::register_local_host_process_service,
            host_terminal_service::register_local_host_terminal_service,
        },
        persistence::backends::{
            minidb::mini_db_query_store::register_mini_db_query_store,
            node_fs::{
                append_log_store::register_append_log_store,
                atomic_document_store::register_atomic_document_stores,
                blob_store_service::register_blob_store_service,
                workspace_local_config_service::register_workspace_local_config_service,
            },
        },
        session::{
            agent_lifecycle::register_agent_lifecycle_service,
            agent_profile_catalog::{
                register_explicit_file_agent_source, register_extra_file_agent_source,
                register_project_file_agent_source, register_session_agent_profile_catalog,
            },
            approval::register_session_approval_service,
            btw::register_session_btw_service,
            cron::register_session_cron_service,
            external_hooks::register_session_external_hooks_service,
            interaction::register_session_interaction_service,
            mcp::register_session_mcp_service,
            process::register_session_process_runner,
            question::register_session_question_service,
            session_init::register_session_init_service,
            session_log::register_session_log_service,
            session_metadata::register_session_metadata,
            skill_catalog::{
                register_explicit_file_skill_source, register_extra_file_skill_source,
                register_plugin_skill_source, register_session_skill_catalog,
                register_workspace_file_skill_source,
            },
            subagent::register_session_subagent_service,
            swarm::register_session_swarm_service,
            terminal::register_session_terminal_service,
            todo::register_session_todo_service,
            tool_policy::register_session_tool_policy,
            workspace_context::register_session_workspace_context,
        },
        wire::register_wire_service,
    };

    register_log_service();
    register_append_log_store();
    register_atomic_document_stores();
    register_blob_store_service();
    register_mini_db_query_store();
    register_workspace_local_config_service();
    register_local_host_environment_service();
    register_local_host_file_system_service();
    register_local_host_process_service();
    register_local_host_terminal_service();

    register_config_registry();
    register_config_service();
    register_event_service();
    register_telemetry_service();
    register_agent_telemetry_context_service();
    register_flag_registry_service();
    register_flag_service();
    register_oauth_toolkit_service();
    register_oauth_service();
    register_protocol_adapter_registry();
    register_provider_service();
    register_model_service();
    register_provider_discovery_service();
    register_host_request_headers();
    register_model_catalog();
    register_agent_catalog_runtime_options();
    register_agent_profile_catalog_service();
    register_user_file_agent_source();
    register_skill_catalog_runtime_options();
    register_builtin_skill_source();
    register_user_file_skill_source();
    register_plugin_service();
    register_workspace_persistence();
    register_workspace_registry_service();
    register_session_index_service();
    register_workspace_query_service();
    register_cron_task_persistence_service();
    register_external_hooks_runner_service();
    register_file_service();
    register_web_fetch_service();
    register_web_search_provider_service();
    register_session_lifecycle_service();
    register_message_legacy_service();

    register_session_interaction_service();
    register_session_approval_service();
    register_session_question_service();
    register_session_metadata();
    register_session_log_service();
    register_session_workspace_context();
    register_session_tool_policy();
    register_session_mcp_service();
    register_explicit_file_skill_source();
    register_extra_file_skill_source();
    register_plugin_skill_source();
    register_workspace_file_skill_source();
    register_session_skill_catalog();
    register_explicit_file_agent_source();
    register_extra_file_agent_source();
    register_project_file_agent_source();
    register_session_agent_profile_catalog();
    register_session_external_hooks_service();
    register_session_cron_service();
    register_session_process_runner();
    register_session_terminal_service();
    register_agent_lifecycle_service();
    register_session_btw_service();
    register_session_subagent_service();
    register_session_swarm_service();
    register_session_todo_service();
    register_session_init_service();

    register_event_bus_service();
    register_agent_blob_service();
    register_wire_service();
    register_agent_context_memory_service();
    register_agent_context_projector_service();
    register_agent_context_size_service();
    register_agent_profile_service();
    register_agent_permission_mode_service();
    register_agent_permission_rules_service();
    register_agent_permission_policy_service();
    register_agent_permission_gate();
    register_agent_tool_registry_service();
    register_agent_tool_policy_service();
    register_tool_result_truncation_service();
    register_agent_tool_executor_service();
    register_agent_llm_requester_service();
    register_usage_service();
    register_agent_system_reminder_service();
    register_agent_context_injector_service();
    register_fault_injection_service();
    register_agent_swarm_service();
    register_agent_loop_service();
    register_agent_prompt_service();
    register_agent_shell_command_service();
    register_agent_skill_service();
    register_agent_rpc_service();
    register_agent_tool_select_service();
    register_agent_tool_select_announcements_service();
    register_agent_step_retry_service();
    register_agent_loop_continuation_service();
    register_agent_task_service();
    register_agent_user_tool_service();
    register_agent_full_compaction_service();
    register_goal_deadline_scheduler_service();
    register_agent_goal_service();
    register_agent_plan_service();
    register_agent_mcp_service();
    register_agent_external_hooks_service();
    register_agent_plugin_service();
    register_agent_media_tools_registrar();
    register_image_config_bridge();
    register_agent_tool_dedupe_service();
    register_agent_builtin_tools_registrar();
    register_agent_activity_view_service();
}
