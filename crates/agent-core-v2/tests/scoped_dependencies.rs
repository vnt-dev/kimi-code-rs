use std::{collections::HashMap, sync::Arc};

use futures_util::StreamExt;
use kimi_code_agent_core_v2::{
    _base::{
        di::{
            lifecycle::Disposable,
            scope::{LifecycleScope, ScopeOptions},
            service_collection::ServiceCollection,
        },
        log::{LOG_OPTIONS_ID, register_log_service, resolve_logging_config},
    },
    app::{
        bootstrap::{BootstrapInput, bootstrap_with_extra},
        event::{
            event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID},
            register_event_bus_service,
        },
    },
    persistence::{
        backends::{
            minidb::mini_db_query_store::register_mini_db_query_store,
            node_fs::{
                append_log_store::register_append_log_store,
                atomic_document_store::register_atomic_document_stores,
                blob_store_service::register_blob_store_service,
            },
        },
        interface::{
            append_log_store::{APPEND_LOG_STORE_SERVICE_ID, AppendLogOptions},
            atomic_document_store::{
                ATOMIC_DOCUMENT_STORE_SERVICE_ID, ATOMIC_TOML_DOCUMENT_STORE_SERVICE_ID,
            },
            blob_store::BLOB_STORE_SERVICE_ID,
            query_store::QUERY_STORE_SERVICE_ID,
        },
    },
};
use serde_json::{Map, Value, json};

#[tokio::test]
async fn bootstrap_scope_resolves_and_runs_all_migrated_dependencies() {
    register_log_service();
    register_append_log_store();
    register_atomic_document_stores();
    register_blob_store_service();
    register_mini_db_query_store();
    register_event_bus_service();

    let home = std::env::temp_dir().join(format!("kimi-scoped-deps-{}", uuid::Uuid::new_v4()));
    let input = BootstrapInput {
        home_dir: Some(home.clone()),
        os_home_dir: Some(home.clone()),
        cwd: Some(home.clone()),
        env: Some(HashMap::new()),
        ..BootstrapInput::default()
    };
    let logging = resolve_logging_config(&home, &HashMap::new());
    let mut extra = ServiceCollection::new();
    extra.set_instance(LOG_OPTIONS_ID, Arc::new(logging));

    let result = bootstrap_with_extra(input, extra).expect("bootstrap must create the App scope");
    let app = result.app;

    let json_store = app
        .get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)
        .expect("JSON store must resolve");
    json_store
        .set("integration", "document.json", &json!({"ready": true}))
        .await
        .unwrap();
    assert_eq!(
        json_store
            .get::<Value>("integration", "document.json")
            .await
            .unwrap(),
        Some(json!({"ready": true}))
    );

    let toml_store = app
        .get(ATOMIC_TOML_DOCUMENT_STORE_SERVICE_ID)
        .expect("TOML store must resolve");
    toml_store
        .set("integration", "document.toml", &json!({"ready": true}))
        .await
        .unwrap();
    assert_eq!(
        toml_store
            .get::<Value>("integration", "document.toml")
            .await
            .unwrap(),
        Some(json!({"ready": true}))
    );

    let blobs = app
        .get(BLOB_STORE_SERVICE_ID)
        .expect("blob store must resolve");
    blobs.0.put("integration", "blob", b"scope").await.unwrap();
    assert_eq!(
        blobs.0.get("integration", "blob").await.unwrap(),
        Some(b"scope".to_vec())
    );

    let append_log = app
        .get(APPEND_LOG_STORE_SERVICE_ID)
        .expect("append-log store must resolve");
    append_log
        .append(
            "integration",
            "events.jsonl",
            &json!({"seq": 1}),
            AppendLogOptions::default(),
        )
        .unwrap();
    append_log.flush().await.unwrap();
    assert_eq!(
        append_log
            .read::<Value>("integration", "events.jsonl")
            .next()
            .await
            .unwrap()
            .unwrap(),
        json!({"seq": 1})
    );

    let query_store = app
        .get(QUERY_STORE_SERVICE_ID)
        .expect("query store must resolve");
    query_store
        .put("documents", "one", &json!({"ready": true}))
        .await
        .unwrap();
    assert_eq!(
        query_store.get::<Value>("documents", "one").await.unwrap(),
        Some(json!({"ready": true}))
    );

    let session = app
        .create_child(
            LifecycleScope::Session,
            "integration-session",
            ScopeOptions::default(),
        )
        .unwrap();
    let agent = session
        .create_child(
            LifecycleScope::Agent,
            "integration-agent",
            ScopeOptions::default(),
        )
        .unwrap();
    let event_bus = agent
        .get(EVENT_BUS_SERVICE_ID)
        .expect("Agent event bus must resolve");
    event_bus.publish(DomainEvent::new("integration.ready", Map::new()));

    query_store.0.close().await.unwrap();
    append_log.close().await.unwrap();
    agent.dispose().unwrap();
    session.dispose().unwrap();
    app.dispose().unwrap();
    tokio::fs::remove_dir_all(home).await.unwrap();
}
