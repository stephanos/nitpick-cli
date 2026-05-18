use std::sync::{Arc, Condvar, Mutex};

use nitpick_agent_core::AgentResult;

#[derive(Clone)]
pub(crate) struct ReviewSlotManager {
    state: Arc<(Mutex<usize>, Condvar)>,
    max_slots: usize,
}

impl ReviewSlotManager {
    pub(crate) fn new(max_slots: usize) -> Self {
        Self {
            state: Arc::new((Mutex::new(0), Condvar::new())),
            max_slots,
        }
    }

    pub(crate) fn try_acquire(&self) -> AgentResult<bool> {
        let (running, _) = self.state.as_ref();
        let mut running = running
            .lock()
            .map_err(|_| nitpick_agent_core::AgentError::io("review slots lock", "poisoned"))?;
        let limit = self.max_slots.max(1);
        if *running >= limit {
            return Ok(false);
        }
        *running += 1;
        Ok(true)
    }

    pub(crate) fn wait_and_acquire(&self) -> AgentResult<()> {
        let (running, changed) = self.state.as_ref();
        let mut running = running
            .lock()
            .map_err(|_| nitpick_agent_core::AgentError::io("review slots lock", "poisoned"))?;
        let limit = self.max_slots.max(1);
        while *running >= limit {
            running = changed
                .wait(running)
                .map_err(|_| nitpick_agent_core::AgentError::io("review slots lock", "poisoned"))?;
        }
        *running += 1;
        Ok(())
    }

    pub(crate) fn release(&self) -> AgentResult<()> {
        let (running, changed) = self.state.as_ref();
        let mut running = running
            .lock()
            .map_err(|_| nitpick_agent_core::AgentError::io("review slots lock", "poisoned"))?;
        *running = running.saturating_sub(1);
        changed.notify_one();
        Ok(())
    }
}
