//! Interactive Agent application assembled through the scoped DI container.
//!
//! Run:
//! `cargo run -p kimi-code-agent-core-v2 --example agent_app -- "your prompt"`
//!
//! Authenticate first when necessary:
//! `cargo run -p kimi-code-agent-core-v2 --example login`

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex, Once},
};

use kimi_code_agent_core_v2::{
    _base::{
        di::{lifecycle::Disposable, service_collection::ServiceCollection},
        log::{LOG_OPTIONS_ID, register_log_service, resolve_logging_config},
    },
    agent::{
        context_memory::{ContextMessage, PromptOrigin},
        loop_::LoopRunResult,
        profile::BindAgentInput,
        prompt::{AGENT_PROMPT_SERVICE_ID, PromptCompletionState, PromptInput},
    },
    app::{
        auth::{OAuthToolkitContract, OAuthToolkitService},
        bootstrap::{BootstrapInput, bootstrap_with_extra, resolve_bootstrap_options},
        config::{CONFIG_SERVICE_ID, ConfigTarget},
        event::event_bus::EVENT_BUS_SERVICE_ID,
        session_lifecycle::{CreateSessionOptions, SESSION_LIFECYCLE_SERVICE_ID},
    },
    kosong::contract::message::{ContentPart, Message, Role},
    session::agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, MAIN_AGENT_ID},
    wire::contract::WIRE_SERVICE_ID,
};
use kimi_code_oauth::{
    CredentialKind, KIMI_CODE_PROVIDER_NAME, ManagedKimiCodeApplyOptions,
    apply_managed_kimi_code_config, fetch_managed_kimi_code_models,
};
use serde_json::{Map, Value};

type AppResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::main(flavor = "current_thread")]
async fn main() -> AppResult<()> {
    let (argument_prompt, requested_cwd, check_scopes) = read_arguments();
    let check_home = check_scopes.then(|| {
        std::env::temp_dir().join(format!("kimi-agent-app-check-{}", uuid::Uuid::new_v4()))
    });
    let bootstrap_input = match &check_home {
        Some(home_dir) => BootstrapInput {
            home_dir: Some(home_dir.clone()),
            os_home_dir: Some(home_dir.clone()),
            cwd: requested_cwd,
            env: Some(HashMap::new()),
            ..BootstrapInput::default()
        },
        None => BootstrapInput {
            cwd: requested_cwd,
            ..BootstrapInput::default()
        },
    };
    let bootstrap_options = resolve_bootstrap_options(bootstrap_input.clone())?;
    let initial_config = if check_scopes {
        Map::new()
    } else {
        managed_model_config(&bootstrap_options.home_dir).await?
    };
    let work_dir = bootstrap_options.cwd.to_string_lossy().into_owned();

    register_contributions()?;
    register_scoped_services();

    let logging = resolve_logging_config(&bootstrap_options.home_dir, &HashMap::new());
    let mut extra = ServiceCollection::new();
    extra.set_instance(LOG_OPTIONS_ID, Arc::new(logging));
    let app = bootstrap_with_extra(bootstrap_input, extra)?.app;

    let result = if check_scopes {
        check_scope_graph(&app, work_dir).await
    } else {
        run_app(&app, initial_config, argument_prompt, work_dir).await
    };
    let dispose_result = app.dispose();
    let cleanup_result = match check_home {
        Some(check_home) => tokio::fs::remove_dir_all(check_home).await,
        None => Ok(()),
    };
    result?;
    dispose_result?;
    cleanup_result?;
    Ok(())
}

async fn check_scope_graph(
    app: &kimi_code_agent_core_v2::_base::di::scope::Scope,
    work_dir: String,
) -> AppResult<()> {
    use kimi_code_agent_core_v2::session::agent_lifecycle::CreateAgentOptions;

    app.get(CONFIG_SERVICE_ID)?.ready().await?;
    let sessions = app.get(SESSION_LIFECYCLE_SERVICE_ID)?;
    let session_id = format!("agent-app-check-{}", uuid::Uuid::new_v4());
    let session = sessions
        .create(CreateSessionOptions {
            session_id: Some(session_id.clone()),
            work_dir,
            ..CreateSessionOptions::default()
        })
        .await?;
    session
        .get(AGENT_LIFECYCLE_SERVICE_ID)?
        .create(CreateAgentOptions {
            agent_id: Some(MAIN_AGENT_ID.into()),
            ..CreateAgentOptions::default()
        })
        .await?;
    sessions.close(&session_id).await?;
    println!("App, Session, and Agent scopes resolved successfully.");
    Ok(())
}

async fn run_app(
    app: &kimi_code_agent_core_v2::_base::di::scope::Scope,
    initial_config: Map<String, Value>,
    argument_prompt: Option<String>,
    work_dir: String,
) -> AppResult<()> {
    let config = app.get(CONFIG_SERVICE_ID)?;
    config.ready().await?;
    for (section, value) in initial_config {
        config
            .replace(&section, Some(value), ConfigTarget::Memory)
            .await?;
    }

    let sessions = app.get(SESSION_LIFECYCLE_SERVICE_ID)?;
    let session_id = format!("agent-app-{}", uuid::Uuid::new_v4());
    let session = sessions
        .create(CreateSessionOptions {
            session_id: Some(session_id.clone()),
            work_dir: work_dir.clone(),
            main_agent_binding: Some(BindAgentInput {
                profile: "agent".into(),
                model: None,
                thinking: None,
                strict_thinking: None,
                cwd: Some(work_dir),
            }),
            ..CreateSessionOptions::default()
        })
        .await?;

    let agents = session.get(AGENT_LIFECYCLE_SERVICE_ID)?;
    let agent = agents
        .get(MAIN_AGENT_ID)
        .ok_or("the session did not create its main agent")?;
    let event_bus = agent.get(EVENT_BUS_SERVICE_ID)?;
    let _interaction_handler = install_interaction_handler(&session)?;
    let _assistant_output = event_bus.subscribe_type(
        "assistant.delta",
        Arc::new(|event| {
            if let Some(text) = event.fields.get("delta").and_then(Value::as_str) {
                print!("{text}");
                let _ = io::stdout().flush();
            }
        }),
    );
    let prompt_service = agent.get(AGENT_PROMPT_SERVICE_ID)?;

    let one_shot = argument_prompt.is_some();
    let mut next_prompt = argument_prompt;
    loop {
        let prompt = match next_prompt.take() {
            Some(prompt) => prompt,
            None => match read_prompt()? {
                Some(prompt) => prompt,
                None => break,
            },
        };
        let handle = prompt_service
            .enqueue(PromptInput {
                id: None,
                message: ContextMessage {
                    message: Message::new(
                        Role::User,
                        vec![ContentPart::Text { text: prompt }],
                        Vec::new(),
                    ),
                    id: None,
                    provider_message_id: None,
                    origin: Some(PromptOrigin::User),
                    is_error: None,
                    note: None,
                },
            })
            .await?;
        let completion = handle.completion().await;
        println!();
        match completion.result {
            Some(LoopRunResult::Completed { steps, truncated }) => {
                eprintln!("Agent completed: steps={steps}, truncated={truncated}");
            }
            Some(LoopRunResult::Failed { error, .. }) => return Err(error.into()),
            Some(LoopRunResult::Cancelled { reason, .. }) => return Err(reason.into()),
            None if completion.state == PromptCompletionState::Blocked => {
                eprintln!("The prompt was blocked by a submission hook.");
            }
            None => eprintln!("The prompt did not launch: {:?}", completion.state),
        }
        agent.get(WIRE_SERVICE_ID)?.flush().await?;

        if one_shot {
            break;
        }
    }

    sessions.close(&session_id).await?;
    Ok(())
}

fn install_interaction_handler(
    session: &kimi_code_agent_core_v2::_base::di::scope::ScopeHandle,
) -> AppResult<kimi_code_agent_core_v2::_base::di::lifecycle::DisposableHandle> {
    use kimi_code_agent_core_v2::session::interaction::SESSION_INTERACTION_SERVICE_ID;

    let interaction = session.get(SESSION_INTERACTION_SERVICE_ID)?;
    let processing = Arc::new(Mutex::new(HashSet::<String>::new()));
    let callback_interaction = interaction.clone();
    let callback_processing = Arc::clone(&processing);
    Ok(interaction.on_did_change_pending().subscribe(move |event| {
        for id in &event.pending {
            if !callback_processing.lock().unwrap().insert(id.clone()) {
                continue;
            }
            let interaction = callback_interaction.clone();
            let processing = Arc::clone(&callback_processing);
            let id = id.clone();
            tokio::spawn(async move {
                let pending = interaction
                    .list_pending(None)
                    .await
                    .into_iter()
                    .find(|candidate| candidate.id == id);
                if let Some(pending) = pending {
                    let response = tokio::task::spawn_blocking(move || {
                        prompt_for_interaction(pending.kind, pending.payload)
                    })
                    .await
                    .unwrap_or(Value::Null);
                    interaction.respond(&id, response).await;
                }
                processing.lock().unwrap().remove(&id);
            });
        }
    }))
}

fn prompt_for_interaction(
    kind: kimi_code_agent_core_v2::session::interaction::InteractionKind,
    payload: Value,
) -> Value {
    use kimi_code_agent_core_v2::session::{
        approval::{ApprovalDecision, ApprovalRequest, ApprovalResponse, ApprovalScope},
        interaction::InteractionKind,
        question::{QuestionAnswer, QuestionAnswerMethod, QuestionRequest, QuestionResponse},
    };

    match kind {
        InteractionKind::Approval => {
            let Ok(request) = serde_json::from_value::<ApprovalRequest>(payload) else {
                return Value::Null;
            };
            eprintln!();
            eprintln!("Tool approval requested: {}", request.action);
            eprintln!("Tool: {}", request.tool_name);
            eprint!("Allow? [y] once / [a] session / [n] reject / [c] cancel: ");
            let _ = io::stderr().flush();
            let mut input = String::new();
            let answer = io::stdin()
                .read_line(&mut input)
                .map(|_| input.trim().to_ascii_lowercase())
                .unwrap_or_else(|_| "c".into());
            let response = match answer.as_str() {
                "y" | "yes" => ApprovalResponse {
                    decision: ApprovalDecision::Approved,
                    scope: None,
                    feedback: None,
                    selected_label: Some("Approve once".into()),
                },
                "a" | "always" => ApprovalResponse {
                    decision: ApprovalDecision::Approved,
                    scope: Some(ApprovalScope::Session),
                    feedback: None,
                    selected_label: Some("Approve for this session".into()),
                },
                "n" | "no" => ApprovalResponse {
                    decision: ApprovalDecision::Rejected,
                    scope: None,
                    feedback: None,
                    selected_label: Some("Reject".into()),
                },
                _ => ApprovalResponse {
                    decision: ApprovalDecision::Cancelled,
                    scope: None,
                    feedback: None,
                    selected_label: Some("Cancel".into()),
                },
            };
            serde_json::to_value(response).unwrap_or(Value::Null)
        }
        InteractionKind::Question => {
            let Ok(request) = serde_json::from_value::<QuestionRequest>(payload) else {
                return Value::Null;
            };
            let mut answers = HashMap::new();
            for question in request.questions {
                eprintln!();
                eprintln!("{}", question.question);
                for (index, option) in question.options.iter().enumerate() {
                    eprintln!("  {}. {}", index + 1, option.label);
                }
                eprint!("Answer: ");
                let _ = io::stderr().flush();
                let mut input = String::new();
                if io::stdin().read_line(&mut input).is_err() {
                    return Value::Null;
                }
                let input = input.trim();
                if input.is_empty() {
                    return Value::Null;
                }
                let answer = input
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| question.options.get(index))
                    .map(|option| option.label.clone())
                    .unwrap_or_else(|| input.to_owned());
                answers.insert(question.question, QuestionAnswer::Text(answer));
            }
            serde_json::to_value(QuestionResponse {
                answers,
                method: Some(QuestionAnswerMethod::Enter),
            })
            .unwrap_or(Value::Null)
        }
        InteractionKind::UserTool => serde_json::json!({
            "output": "This example has no host implementation for the requested user tool.",
            "isError": true,
        }),
    }
}

async fn managed_model_config(home_dir: &std::path::Path) -> AppResult<Map<String, Value>> {
    let oauth = OAuthToolkitService::new(home_dir)?;
    let access_token = oauth
        .get_cached_access_token(Some(KIMI_CODE_PROVIDER_NAME), None)
        .await?
        .ok_or("not logged in; run `cargo run -p kimi-code-agent-core-v2 --example login` first")?;
    let models =
        fetch_managed_kimi_code_models(&access_token, None, None, CredentialKind::OAuth).await?;
    if models.is_empty() {
        return Err("the current account has no available models".into());
    }
    let mut config = Map::new();
    apply_managed_kimi_code_config(
        &mut config,
        ManagedKimiCodeApplyOptions {
            models: &models,
            base_url: None,
            oauth_key: None,
            oauth_host: None,
            preserve_default_model: false,
        },
    )?;
    Ok(config)
}

fn read_arguments() -> (Option<String>, Option<PathBuf>, bool) {
    let mut cwd = None;
    let mut check_scopes = false;
    let mut prompt = Vec::new();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--cwd" {
            cwd = arguments.next().map(PathBuf::from);
        } else if argument == "--check-scopes" {
            check_scopes = true;
        } else {
            prompt.push(argument);
        }
    }
    (
        (!prompt.is_empty()).then(|| prompt.join(" ")),
        cwd,
        check_scopes,
    )
}

fn read_prompt() -> io::Result<Option<String>> {
    loop {
        print!("Prompt (Ctrl+Z/Ctrl+D to exit): ");
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Ok(None);
        }
        let input = input.trim();
        if !input.is_empty() {
            return Ok(Some(input.to_owned()));
        }
    }
}

fn register_contributions() -> AppResult<()> {
    static REGISTER: Once = Once::new();
    let mut result = Ok(());
    REGISTER.call_once(|| {
        result = register_contributions_once();
    });
    result
}

fn register_contributions_once() -> AppResult<()> {
    use kimi_code_agent_core_v2::{
        agent::{
            external_hooks::register_hooks_config_section,
            loop_::register_loop_control_config_section, media::register_image_config_section,
            permission_mode::register_default_permission_mode_config_section,
            permission_rules::register_permission_config_section,
            plan::register_default_plan_mode_config_section, task::register_task_config_sections,
            tool_policy::register_tools_config_section,
        },
        app::{
            agent_file_catalog::register_agent_file_catalog_config_sections,
            auth::register_services_config_section, cron::register_cron_config_section,
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

    ensure_provider_definitions_registered()?;
    ensure_openai_legacy_base_registered()?;
    ensure_openai_responses_base_registered()?;
    ensure_anthropic_base_registered()?;
    ensure_google_gen_ai_base_registered()?;

    kimi_code_agent_core_v2::agent::tool_select::register_select_tools_tool();
    kimi_code_agent_core_v2::app::web::register_fetch_url_tool();
    kimi_code_agent_core_v2::agent::task::tools::register_task_list_tool();
    kimi_code_agent_core_v2::agent::task::tools::register_task_output_tool();
    kimi_code_agent_core_v2::agent::task::tools::register_task_stop_tool();
    kimi_code_agent_core_v2::os::backends::node_local::tools::register_node_local_tools();
    Ok(())
}

fn register_scoped_services() {
    use kimi_code_agent_core_v2::{
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
            auth::{register_oauth_service, register_oauth_toolkit_service},
            config::{register_config_registry, register_config_service},
            cron::register_cron_task_persistence_service,
            event::{register_event_bus_service, register_event_service},
            external_hooks_runner::register_external_hooks_runner_service,
            file::register_file_service,
            flag::{register_flag_registry_service, register_flag_service},
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
                register_workspace_persistence, register_workspace_registry_service,
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
    register_cron_task_persistence_service();
    register_external_hooks_runner_service();
    register_file_service();
    register_web_fetch_service();
    register_session_lifecycle_service();

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
