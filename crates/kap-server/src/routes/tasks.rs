use super::{RouteSpec, route};
use crate::web::CoreOperation;

// Original: packages/kap-server/src/routes/tasks.ts, registerTasksRoutes().
pub const ROUTES: &[RouteSpec] = &[
    route(
        "GET",
        "/api/v1/sessions/{session_id}/tasks",
        "/api/v1/sessions/{session_id}/tasks",
        CoreOperation::ListTasks,
    ),
    route(
        "GET",
        "/api/v1/sessions/{session_id}/tasks/{item}",
        "/api/v1/sessions/{session_id}/tasks/{task_id}",
        CoreOperation::GetTask,
    ),
    route(
        "POST",
        "/api/v1/sessions/{session_id}/tasks/{item}",
        "/api/v1/sessions/{session_id}/tasks/{tail}",
        CoreOperation::TaskAction,
    ),
];
