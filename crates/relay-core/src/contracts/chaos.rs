use crate::contracts::JsonValue;
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum InjectedFaultKind {
    MissingDependency,
    UpstreamCodeEdit,
    SameLineConflict,
    MidRunDisconnect,
    RuntimeException,
    CorruptedEnvLock,
    StaleEvidenceTrap,
    DuplicateTaskReplay,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct InjectedFault {
    pub kind: InjectedFaultKind,
    pub target_path: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Type)]
pub struct ChaosAssertion {
    pub assertion_id: String,
    pub description: String,
    pub metric_key: String,
    pub expected_value: JsonValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Type)]
pub struct ChaosCase {
    pub case_id: String,
    pub title: String,
    pub description: String,
    pub initial_commit: String,
    pub injected_fault: InjectedFault,
    pub user_prompt: String,
    pub expected_tool_trace: Vec<String>,
    pub forbidden_hallucinations: Vec<String>,
    pub ground_truth_assertions: Vec<ChaosAssertion>,
}
