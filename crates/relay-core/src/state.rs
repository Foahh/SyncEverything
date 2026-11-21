use crate::contracts::{TaskId, TaskState};
use crate::error::{RelayError, RelayResult};

pub fn validate_transition(task_id: TaskId, from: TaskState, to: TaskState) -> RelayResult<()> {
    use TaskState::*;

    match (from, to) {
        (Queued, PreflightRunning | Executing | Failed | Rejected)
        | (PreflightRunning, SelfHealingProposed | Executing | Completed | Failed | Rejected)
        | (SelfHealingProposed, Executing | Rejected | Failed)
        | (Executing, Completed | Failed) => Ok(()),
        (_, Stale) if from != Rejected => Ok(()),
        _ => Err(RelayError::InvalidStateTransition {
            task_id: task_id.to_string(),
            from: format!("{from:?}"),
            to: format!("{to:?}"),
        }),
    }
}

pub(crate) fn is_active(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Queued
            | TaskState::PreflightRunning
            | TaskState::SelfHealingProposed
            | TaskState::Executing
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use TaskState::*;

    #[test]
    fn enforces_every_state_pair() {
        let task_id = TaskId::now_v7();
        let allowed: [(TaskState, &[TaskState]); 8] = [
            (Queued, &[PreflightRunning, Executing, Failed, Stale, Rejected]),
            (
                PreflightRunning,
                &[SelfHealingProposed, Executing, Completed, Failed, Stale, Rejected],
            ),
            (SelfHealingProposed, &[Executing, Rejected, Stale, Failed]),
            (Executing, &[Completed, Failed, Stale]),
            (Completed, &[Stale]),
            (Failed, &[Stale]),
            (Stale, &[Stale]),
            (Rejected, &[]),
        ];
        for (from, permitted) in allowed {
            for (to, _) in allowed {
                assert_eq!(
                    validate_transition(task_id, from, to).is_ok(),
                    permitted.contains(&to),
                    "{from:?} -> {to:?}",
                );
            }
        }
    }
}
