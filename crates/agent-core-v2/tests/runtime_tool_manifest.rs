use std::collections::{BTreeSet, HashMap};

use kimi_code_agent_core_v2::{
    _base::di::lifecycle::Disposable,
    agent::{profile::BindAgentInput, tool_registry::AGENT_TOOL_REGISTRY_SERVICE_ID},
    app::{
        agent_app_runtime::bootstrap_agent_app,
        agent_profile_catalog::AGENT_PROFILE_CATALOG_SERVICE_ID,
        bootstrap::BootstrapInput,
        session_lifecycle::{CreateSessionOptions, SESSION_LIFECYCLE_SERVICE_ID},
    },
    kosong::model::ENV_MODEL_ALIAS_KEY,
    session::agent_lifecycle::{AGENT_LIFECYCLE_SERVICE_ID, CreateAgentOptions},
};

#[tokio::test]
async fn default_profile_tools_exist_in_the_runtime_registry() {
    let root = std::env::temp_dir().join(format!(
        "kimi-runtime-tool-manifest-{}",
        uuid::Uuid::new_v4()
    ));
    let home = root.join("home");
    let work_dir = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work_dir).unwrap();
    std::fs::write(
        home.join("config.toml"),
        r#"
[services.moonshot_search]
base_url = "http://127.0.0.1/search"
api_key = "runtime-test"
"#,
    )
    .unwrap();

    let app = bootstrap_agent_app(BootstrapInput {
        home_dir: Some(home.clone()),
        os_home_dir: Some(home.clone()),
        cwd: Some(work_dir.clone()),
        env: Some(HashMap::from([
            ("KIMI_MODEL_NAME".into(), "runtime-test-model".into()),
            ("KIMI_MODEL_PROVIDER_TYPE".into(), "kimi".into()),
            ("KIMI_MODEL_API_KEY".into(), "runtime-test".into()),
            ("KIMI_MODEL_BASE_URL".into(), "http://127.0.0.1/v1".into()),
            ("KIMI_MODEL_CAPABILITIES".into(), "image_in,tool_use".into()),
        ])),
        client_version: Some("runtime-tool-manifest-test".into()),
        ..BootstrapInput::default()
    })
    .expect("the complete Agent runtime must bootstrap");

    let profiles = app
        .get(AGENT_PROFILE_CATALOG_SERVICE_ID)
        .expect("the runtime profile catalog must resolve");
    let profile = profiles
        .get_default()
        .expect("the runtime must provide its default profile");
    let declared_tools = profile
        .tools
        .as_ref()
        .expect("the default profile must declare its tool allowlist");

    let sessions = app
        .get(SESSION_LIFECYCLE_SERVICE_ID)
        .expect("the runtime session lifecycle must resolve");
    let session = sessions
        .create(CreateSessionOptions {
            session_id: Some("runtime-tool-manifest".into()),
            work_dir: work_dir.to_string_lossy().into_owned(),
            ..CreateSessionOptions::default()
        })
        .await
        .expect("the runtime session must start");
    let agents = session
        .get(AGENT_LIFECYCLE_SERVICE_ID)
        .expect("the session Agent lifecycle must resolve");
    let agent = agents
        .create(CreateAgentOptions {
            agent_id: Some("main".into()),
            binding: Some(BindAgentInput {
                profile: "agent".into(),
                model: Some(ENV_MODEL_ALIAS_KEY.into()),
                thinking: None,
                strict_thinking: None,
                cwd: Some(work_dir.to_string_lossy().into_owned()),
            }),
            ..CreateAgentOptions::default()
        })
        .await
        .expect("the runtime Agent must start");
    let registry = agent
        .get(AGENT_TOOL_REGISTRY_SERVICE_ID)
        .expect("the runtime Tool Registry must resolve");

    let mut missing = BTreeSet::new();
    let mut selectors = BTreeSet::new();
    for declaration in declared_tools {
        if declaration.contains(['*', '?', '[']) {
            selectors.insert(declaration.clone());
        } else if registry.resolve(declaration).is_none() {
            missing.insert(declaration.clone());
        }
    }
    let registered = registry
        .list_references()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();

    sessions
        .close(session.id())
        .await
        .expect("the runtime session must close");
    app.dispose().expect("the runtime App must dispose");
    tokio::fs::remove_dir_all(&root).await.unwrap();

    assert_eq!(
        selectors,
        BTreeSet::from(["mcp__*".to_owned()]),
        "the default profile may use the dynamic MCP selector, but no other unresolved patterns"
    );
    assert!(
        missing.is_empty(),
        "default profile tools missing from the runtime Tool Registry: {missing:?}; registered tools: {registered:?}"
    );
}
