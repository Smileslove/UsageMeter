//! Transaction-like compensation helpers for gateway operations.
//!
//! Since we store secrets in the OS keychain and metadata in JSON settings,
//! we need explicit compensation logic to maintain consistency.

use std::sync::Mutex;

/// A compensation action that can be rolled back.
pub trait CompensationAction: Send {
    fn rollback(&self);
    fn description(&self) -> &str;
}

/// A transaction-like scope that tracks compensation actions.
/// If dropped without commit, all actions are rolled back in reverse order.
pub struct CompensationScope {
    actions: Mutex<Vec<Box<dyn CompensationAction>>>,
    committed: Mutex<bool>,
}

impl CompensationScope {
    pub fn new() -> Self {
        Self {
            actions: Mutex::new(Vec::new()),
            committed: Mutex::new(false),
        }
    }

    /// Register a compensation action to be executed on rollback.
    pub fn register(&self, action: Box<dyn CompensationAction>) {
        if let Ok(mut actions) = self.actions.lock() {
            actions.push(action);
        }
    }

    /// Commit the transaction, preventing rollback on drop.
    pub fn commit(&self) {
        if let Ok(mut committed) = self.committed.lock() {
            *committed = true;
        }
    }

    /// Explicitly rollback all registered actions.
    pub fn rollback(&self) {
        if let Ok(mut actions) = self.actions.lock() {
            // Rollback in reverse order
            for action in actions.drain(..).rev() {
                log::debug!("Rolling back: {}", action.description());
                action.rollback();
            }
        }
    }
}

impl Default for CompensationScope {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CompensationScope {
    fn drop(&mut self) {
        let should_rollback = self
            .committed
            .lock()
            .map(|c| !*c)
            .unwrap_or(false);

        if should_rollback {
            self.rollback();
        }
    }
}

/// Compensation action for deleting a secret from the OS credential store.
pub struct DeleteSecretAction {
    pub secret_ref: String,
}

impl CompensationAction for DeleteSecretAction {
    fn rollback(&self) {
        crate::gateway::delete_upstream_secret(&self.secret_ref);
    }

    fn description(&self) -> &str {
        "delete secret from credential store"
    }
}

/// Compensation action for clearing profile runtime state.
pub struct ClearProfileStateAction {
    pub profile_id: String,
}

impl CompensationAction for ClearProfileStateAction {
    fn rollback(&self) {
        crate::gateway::clear_profile_runtime_state(&self.profile_id);
    }

    fn description(&self) -> &str {
        "clear profile runtime state"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TestAction {
        rolled_back: Arc<AtomicBool>,
    }

    impl CompensationAction for TestAction {
        fn rollback(&self) {
            self.rolled_back.store(true, Ordering::SeqCst);
        }

        fn description(&self) -> &str {
            "test action"
        }
    }

    #[test]
    fn rollback_on_drop() {
        let rolled_back = Arc::new(AtomicBool::new(false));
        {
            let scope = CompensationScope::new();
            scope.register(Box::new(TestAction {
                rolled_back: rolled_back.clone(),
            }));
            // Drop without commit
        }
        assert!(rolled_back.load(Ordering::SeqCst));
    }

    #[test]
    fn no_rollback_on_commit() {
        let rolled_back = Arc::new(AtomicBool::new(false));
        {
            let scope = CompensationScope::new();
            scope.register(Box::new(TestAction {
                rolled_back: rolled_back.clone(),
            }));
            scope.commit();
        }
        assert!(!rolled_back.load(Ordering::SeqCst));
    }
}
