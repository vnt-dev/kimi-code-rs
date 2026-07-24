use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/questions.ts, registerQuestionsRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/questions",
        "/api/v1/sessions/{session_id}/questions",
        CoreOperation::ListQuestions,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/questions/{tail}",
        "/api/v1/sessions/{session_id}/questions/{tail}",
        CoreOperation::QuestionAction,
    ),
];
