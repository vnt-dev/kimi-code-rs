use std::{collections::BTreeMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    cli::prompt_session::{
        QuestionAnswer, QuestionAnswerMethod, QuestionHandler, QuestionRequest, QuestionResult,
    },
    tui::reverse_rpc::types::{
        QuestionPanelData, QuestionPanelItem, QuestionPanelOption, QuestionPanelResponse,
        QuestionSubmissionMethod,
    },
};

use super::controller::QuestionController;

// Original:
//   apps/kimi-code/src/tui/reverse-rpc/question/handler.ts
//   createQuestionAskHandler()
pub fn create_question_ask_handler(controller: Arc<Mutex<QuestionController>>) -> QuestionHandler {
    Arc::new(move |event| {
        let controller = Arc::clone(&controller);
        Box::pin(async move {
            let receiver = {
                let mut controller = controller.lock().await;
                controller.show(adapt_question_request(&event))
            };
            let response = receiver.await.ok()?;
            adapt_question_answers(&event, &response)
        })
    })
}

// Original:
//   apps/kimi-code/src/tui/reverse-rpc/question/handler.ts
//   adaptQuestionRequest()
pub fn adapt_question_request(event: &QuestionRequest) -> QuestionPanelData {
    let id = event.tool_call_id.clone().unwrap_or_else(|| {
        event.turn_id.map_or_else(
            || "question".to_owned(),
            |turn_id| format!("question-{turn_id}"),
        )
    });
    QuestionPanelData {
        id: id.clone(),
        tool_call_id: id,
        questions: event
            .questions
            .iter()
            .map(|question| QuestionPanelItem {
                question: question.question.clone(),
                header: question.header.clone(),
                body: question.body.clone(),
                multi_select: question.multi_select,
                other_label: question.other_label.clone(),
                other_description: question.other_description.clone(),
                options: question
                    .options
                    .iter()
                    .map(|option| QuestionPanelOption {
                        label: option.label.clone(),
                        description: option.description.clone(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

// Original: adaptQuestionAnswers()
pub fn adapt_question_answers(
    event: &QuestionRequest,
    response: &QuestionPanelResponse,
) -> Option<QuestionResult> {
    let answers = event
        .questions
        .iter()
        .zip(response.answers.iter())
        .filter_map(|(question, answer)| {
            let answer = answer.as_ref().filter(|answer| !answer.is_empty())?;
            Some((
                question.question.clone(),
                QuestionAnswer::Text(answer.clone()),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    if answers.is_empty() {
        return None;
    }
    Some(QuestionResult::Response {
        answers,
        method: response.method.map(map_submission_method),
    })
}

fn map_submission_method(method: QuestionSubmissionMethod) -> QuestionAnswerMethod {
    match method {
        QuestionSubmissionMethod::Enter => QuestionAnswerMethod::Enter,
        QuestionSubmissionMethod::Space => QuestionAnswerMethod::Space,
        QuestionSubmissionMethod::NumberKey => QuestionAnswerMethod::NumberKey,
    }
}

#[cfg(test)]
mod tests {
    use crate::cli::prompt_session::{QuestionItem, QuestionOption};

    use super::*;

    fn request() -> QuestionRequest {
        QuestionRequest {
            turn_id: Some(7),
            tool_call_id: None,
            questions: vec![
                QuestionItem {
                    question: "Q1?".to_owned(),
                    header: Some("Pick".to_owned()),
                    body: Some("Choose one".to_owned()),
                    options: vec![QuestionOption {
                        label: "Alpha".to_owned(),
                        description: Some("First option".to_owned()),
                    }],
                    multi_select: true,
                    other_label: Some("Other".to_owned()),
                    other_description: Some("Custom".to_owned()),
                },
                QuestionItem {
                    question: "Storage?".to_owned(),
                    header: None,
                    body: None,
                    options: vec![],
                    multi_select: false,
                    other_label: None,
                    other_description: None,
                },
            ],
        }
    }

    #[test]
    fn normalizes_request_and_uses_turn_fallback_id() {
        let panel = adapt_question_request(&request());
        assert_eq!(panel.id, "question-7");
        assert_eq!(panel.tool_call_id, "question-7");
        assert_eq!(panel.questions[0].header.as_deref(), Some("Pick"));
        assert!(panel.questions[0].multi_select);
        assert_eq!(panel.questions[0].options[0].label, "Alpha");
    }

    #[test]
    fn maps_nonempty_answers_by_question_text_and_preserves_holes() {
        let result = adapt_question_answers(
            &request(),
            &QuestionPanelResponse {
                answers: vec![None, Some("SQLite".to_owned())],
                method: Some(QuestionSubmissionMethod::NumberKey),
            },
        )
        .expect("result");
        let QuestionResult::Response { answers, method } = result else {
            panic!("response metadata must be retained");
        };
        assert_eq!(answers.len(), 1);
        assert_eq!(
            answers["Storage?"],
            QuestionAnswer::Text("SQLite".to_owned())
        );
        assert_eq!(method, Some(QuestionAnswerMethod::NumberKey));
    }

    #[test]
    fn empty_answers_return_none_and_tool_call_id_wins() {
        let mut event = request();
        event.tool_call_id = Some("q-1".to_owned());
        assert_eq!(adapt_question_request(&event).id, "q-1");
        assert!(
            adapt_question_answers(
                &event,
                &QuestionPanelResponse {
                    answers: vec![Some(String::new())],
                    method: None,
                }
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn async_handler_waits_for_panel_and_maps_answer() {
        let controller = Arc::new(Mutex::new(QuestionController::new()));
        let handler = create_question_ask_handler(Arc::clone(&controller));
        let task = tokio::spawn(handler(request()));
        tokio::task::yield_now().await;
        controller.lock().await.respond(QuestionPanelResponse {
            answers: vec![Some("Alpha".to_owned()), Some("SQLite".to_owned())],
            method: Some(QuestionSubmissionMethod::Enter),
        });
        let result = task.await.expect("handler task").expect("answers");
        let QuestionResult::Response { answers, method } = result else {
            panic!("response expected");
        };
        assert_eq!(answers.len(), 2);
        assert_eq!(method, Some(QuestionAnswerMethod::Enter));
    }

    #[tokio::test]
    async fn dropped_question_response_returns_none() {
        let controller = Arc::new(Mutex::new(QuestionController::new()));
        let handler = create_question_ask_handler(Arc::clone(&controller));
        let task = tokio::spawn(handler(request()));
        tokio::task::yield_now().await;
        *controller.lock().await = QuestionController::new();
        assert_eq!(task.await.expect("handler task"), None);
    }
}
