//! Typed facade over the session interaction kernel for ask-user requests.
//!
//! Original: `session/question/questionService.ts`, `SessionQuestionService`.

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::abort::AbortSignal,
    },
    session::interaction::{
        InteractionKind, InteractionOrigin, InteractionRequest, SESSION_INTERACTION_SERVICE_ID,
        SessionInteractionService,
    },
};

use super::{
    QuestionRequest, QuestionResult, SESSION_QUESTION_SERVICE_ID, SessionQuestionServiceHandle,
};

#[derive(Clone, Default)]
pub struct QuestionRequestOptions {
    pub signal: Option<AbortSignal>,
    pub agent_id: Option<String>,
}

pub struct SessionQuestionService {
    interaction: Arc<SessionInteractionService>,
}

impl SessionQuestionService {
    pub fn new(interaction: Arc<SessionInteractionService>) -> Self {
        Self { interaction }
    }

    // Original: request(). The interaction kernel owns the parked request.
    pub async fn request(
        &self,
        request: QuestionRequest,
        options: Option<QuestionRequestOptions>,
    ) -> Option<QuestionResult> {
        let id = request_id(&request);
        let agent_id = options
            .as_ref()
            .and_then(|options| options.agent_id.clone());
        let interaction = InteractionRequest {
            id: Some(id.clone()),
            kind: InteractionKind::Question,
            payload: question_request_to_value(&request),
            origin: Some(InteractionOrigin {
                turn_id: request.turn_id.map(|turn_id| turn_id as f64),
                agent_id,
            }),
        };

        let mut pending = Box::pin(self.interaction.begin_request(interaction).await);
        let value = match options.and_then(|options| options.signal) {
            Some(signal) => tokio::select! {
                response = &mut pending => response.unwrap_or(Value::Null),
                _ = signal.cancelled() => {
                    // The source's abort listener runs after an already
                    // delivered answer, so an answer wins that race.
                    if self.interaction.is_recently_resolved(&id).await {
                        pending.await.unwrap_or(Value::Null)
                    } else {
                        self.dismiss(&id).await;
                        Value::Null
                    }
                }
            },
            None => pending.await.unwrap_or(Value::Null),
        };
        serde_json::from_value(value).ok()
    }

    // Original: enqueue().
    pub async fn enqueue(&self, request: QuestionRequest) -> QuestionRequest {
        let id = request_id(&request);
        self.interaction
            .enqueue(InteractionRequest {
                id: Some(id.clone()),
                kind: InteractionKind::Question,
                payload: question_request_to_value(&request),
                origin: Some(InteractionOrigin {
                    turn_id: request.turn_id.map(|turn_id| turn_id as f64),
                    agent_id: None,
                }),
            })
            .await;
        QuestionRequest {
            id: Some(id),
            ..request
        }
    }

    // Original: answer().
    pub async fn answer(&self, id: &str, result: QuestionResult) {
        self.interaction
            .respond(id, question_result_to_value(&result))
            .await;
    }

    // Original: dismiss().
    pub async fn dismiss(&self, id: &str) {
        self.interaction.respond(id, Value::Null).await;
    }

    // Original: listPending().
    pub async fn list_pending(&self) -> Vec<QuestionRequest> {
        self.interaction
            .list_pending(Some(InteractionKind::Question))
            .await
            .into_iter()
            .filter_map(|interaction| serde_json::from_value(interaction.payload).ok())
            .collect()
    }

    pub fn interaction(&self) -> &Arc<SessionInteractionService> {
        &self.interaction
    }
}

// Original: registerScopedService(..., SessionQuestionService, Eager,
// "question").
pub fn register_session_question_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_QUESTION_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let interaction = accessor.get(SESSION_INTERACTION_SERVICE_ID)?;
            Ok(SessionQuestionServiceHandle(Arc::new(
                SessionQuestionService::new(Arc::clone(&interaction.0)),
            )))
        }),
        InstantiationType::Eager,
        "question",
    );
}

fn request_id(request: &QuestionRequest) -> String {
    request
        .id
        .clone()
        .or_else(|| request.tool_call_id.clone())
        .unwrap_or_else(|| format!("question:{}", now_ms()))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn question_request_to_value(request: &QuestionRequest) -> Value {
    serde_json::to_value(request).unwrap_or_default()
}

fn question_result_to_value(result: &QuestionResult) -> Value {
    serde_json::to_value(result).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        _base::{
            di::{
                lifecycle::Disposable,
                scope::{
                    LifecycleScope, Scope, ScopeOptions, clear_scoped_registry_for_tests,
                    get_scoped_service_descriptors,
                },
            },
            utils::abort::AbortController,
        },
        session::interaction::register_session_interaction_service,
        session::question::{
            QuestionAnswer, QuestionAnswerMethod, QuestionItem, QuestionOption, QuestionResponse,
        },
    };

    fn request(id: &str) -> QuestionRequest {
        QuestionRequest {
            id: Some(id.into()),
            turn_id: None,
            tool_call_id: Some(format!("tc-{id}")),
            questions: vec![QuestionItem {
                question: "Pick one".into(),
                header: None,
                body: None,
                options: vec![
                    QuestionOption {
                        label: "Yes".into(),
                        description: None,
                    },
                    QuestionOption {
                        label: "No".into(),
                        description: None,
                    },
                ],
                multi_select: None,
                other_label: None,
                other_description: None,
            }],
        }
    }

    #[test]
    fn registration_matches_the_eager_session_scoped_source_binding() {
        clear_scoped_registry_for_tests();
        register_session_interaction_service();
        register_session_question_service();
        let entries = get_scoped_service_descriptors(LifecycleScope::Session);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == SESSION_QUESTION_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "question"
        }));
        let app = Scope::create_app(ScopeOptions::default());
        let session = app
            .create_child(LifecycleScope::Session, "session", ScopeOptions::default())
            .unwrap();
        session.get(SESSION_QUESTION_SERVICE_ID).unwrap();
        session.dispose().unwrap();
        app.dispose().unwrap();
        clear_scoped_registry_for_tests();
    }

    fn answer(label: &str) -> QuestionResult {
        QuestionResult::Response(QuestionResponse {
            answers: [("q_0".into(), QuestionAnswer::Text(label.into()))]
                .into_iter()
                .collect(),
            method: Some(QuestionAnswerMethod::NumberKey),
        })
    }

    #[tokio::test]
    async fn requests_enqueue_answer_and_dismiss_through_the_interaction_kernel() {
        let interaction = Arc::new(SessionInteractionService::new());
        let questions = Arc::new(SessionQuestionService::new(Arc::clone(&interaction)));
        let pending = {
            let questions = Arc::clone(&questions);
            tokio::spawn(async move { questions.request(request("q1"), None).await })
        };
        tokio::task::yield_now().await;
        assert_eq!(
            questions
                .list_pending()
                .await
                .iter()
                .map(|item| item.id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("q1")]
        );
        questions.answer("q1", answer("Yes")).await;
        assert_eq!(pending.await.unwrap(), Some(answer("Yes")));
        assert!(questions.list_pending().await.is_empty());

        let queued = questions.enqueue(request("q2")).await;
        assert_eq!(queued.id.as_deref(), Some("q2"));
        questions.dismiss("q2").await;
        assert!(interaction.is_recently_resolved("q2").await);
    }

    #[tokio::test]
    async fn cancellation_dismisses_the_parked_question_with_null() {
        let questions = Arc::new(SessionQuestionService::new(Arc::new(
            SessionInteractionService::new(),
        )));
        let controller = AbortController::new();
        let signal = controller.signal();
        let pending = {
            let questions = Arc::clone(&questions);
            tokio::spawn(async move {
                questions
                    .request(
                        request("q1"),
                        Some(QuestionRequestOptions {
                            signal: Some(signal),
                            agent_id: Some("sub-1".into()),
                        }),
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        assert_eq!(
            questions.interaction().list_pending(None).await[0]
                .origin
                .agent_id
                .as_deref(),
            Some("sub-1")
        );
        controller.abort(None);
        assert_eq!(pending.await.unwrap(), None);
        assert!(questions.list_pending().await.is_empty());
    }

    #[tokio::test]
    async fn pre_aborted_request_is_dismissed_without_remaining_pending() {
        let questions = SessionQuestionService::new(Arc::new(SessionInteractionService::new()));
        let controller = AbortController::new();
        controller.abort(None);

        assert_eq!(
            questions
                .request(
                    request("q1"),
                    Some(QuestionRequestOptions {
                        signal: Some(controller.signal()),
                        agent_id: None,
                    }),
                )
                .await,
            None
        );
        assert!(questions.list_pending().await.is_empty());
        assert!(questions.interaction().is_recently_resolved("q1").await);
    }

    #[test]
    fn answer_serialization_preserves_true_marker_and_method() {
        let value = serde_json::to_value(QuestionResponse {
            answers: [("q_0".into(), QuestionAnswer::Selected)]
                .into_iter()
                .collect(),
            method: Some(QuestionAnswerMethod::NumberKey),
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"answers": {"q_0": true}, "method": "number_key"})
        );
        assert!(serde_json::from_value::<QuestionAnswer>(serde_json::json!(false)).is_err());
    }
}
