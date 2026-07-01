use serde::Serialize;

use crate::{DummyExecutionReport, ExecutionPlan, RuntimeIssue};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeApiResponse {
    pub ok: bool,
    pub issues: Vec<RuntimeIssue>,
    pub plan: Option<ExecutionPlan>,
    pub report: Option<DummyExecutionReport>,
}

impl RuntimeApiResponse {
    pub(super) fn issues(issues: Vec<RuntimeIssue>) -> Self {
        Self {
            ok: false,
            issues,
            plan: None,
            report: None,
        }
    }
}
