use std::sync::Mutex;

use nitpick_agent_host::Clock;

pub struct ManualClock {
    now_unix: Mutex<u64>,
}

impl ManualClock {
    pub fn new(now_unix: u64) -> Self {
        Self {
            now_unix: Mutex::new(now_unix),
        }
    }

    pub fn advance(&self, seconds: u64) {
        *self.now_unix.lock().expect("lock") += seconds;
    }
}

impl Clock for ManualClock {
    fn now_unix(&self) -> u64 {
        *self.now_unix.lock().expect("lock")
    }
}
