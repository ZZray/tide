//! Pure scheduling state. Callers supply monotonic seconds, including in tests.
#[derive(Clone, Debug)]
pub struct Schedule {
    pub interval: u64,
    pub debounce: u64,
    pub due: Option<u64>,
    pub running: bool,
    pub disabled: bool,
    pub retry_stopped: bool,
    pub user_paused: bool,
    first_change: Option<u64>,
    change_due: Option<u64>,
    rerun: bool,
    failures: u32,
}
impl Schedule {
    pub fn new(interval: u64, debounce: u64, now: u64, paused: bool) -> Self {
        Self {
            interval,
            debounce,
            due: if interval > 0 { Some(now + 10) } else { None },
            running: false,
            disabled: paused,
            retry_stopped: false,
            user_paused: paused,
            first_change: None,
            change_due: None,
            rerun: false,
            failures: 0,
        }
    }
    pub fn change(&mut self, now: u64) {
        if self.disabled {
            return;
        }
        if self.running {
            self.rerun = true;
            return;
        }
        if self.failures > 0 {
            return;
        } // Edits must not bypass network retry backoff.
        let first = *self.first_change.get_or_insert(now);
        self.change_due = Some((now + self.debounce).min(first + 300));
    }
    pub fn request(&mut self, now: u64) {
        if self.disabled {
            return;
        }
        if self.running {
            self.rerun = true;
        } else {
            self.due = Some(now);
        }
    }
    pub fn next(&self) -> Option<u64> {
        if self.disabled || self.running {
            return None;
        }
        self.due.into_iter().chain(self.change_due).min()
    }
    pub fn start(&mut self, now: u64) -> bool {
        if !self.next().is_some_and(|due| due <= now) {
            return false;
        }
        self.running = true;
        self.due = None;
        self.first_change = None;
        self.change_due = None;
        true
    }
    pub fn finish(&mut self, now: u64, success: bool, stop_retry: bool) {
        self.running = false;
        if stop_retry {
            self.retry_stopped = true;
            self.disabled = true;
            self.rerun = false;
            return;
        }
        if success {
            self.failures = 0;
            self.due = if self.rerun {
                Some(now)
            } else if self.interval > 0 {
                Some(now + self.interval)
            } else {
                None
            };
        } else {
            self.failures = self.failures.saturating_add(1);
            self.due = Some(
                now + (60u64.saturating_mul(1 << self.failures.min(6).saturating_sub(1))).min(1800),
            );
        }
        self.rerun = false;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_debounce() {
        let mut s = Schedule::new(0, 30, 0, false);
        for t in (0..300).step_by(10) {
            s.change(t);
        }
        assert_eq!(s.next(), Some(300));
        assert!(s.start(300));
        assert!(!s.start(300));
    }
    #[test]
    fn coalesces_and_backs_off() {
        let mut s = Schedule::new(900, 30, 0, false);
        assert!(s.start(10));
        for _ in 0..10 {
            s.request(11);
        }
        s.finish(12, true, false);
        assert_eq!(s.next(), Some(12));
        assert!(s.start(12));
        s.finish(13, false, false);
        assert_eq!(s.next(), Some(73));
        assert!(s.start(73));
        s.finish(74, false, false);
        assert_eq!(s.next(), Some(194));
        assert!(s.start(194));
        s.finish(195, false, true);
        assert_eq!(s.next(), None);
    }
    #[test]
    fn pause_and_wakeup() {
        let mut s = Schedule::new(900, 30, 0, true);
        s.request(100);
        assert_eq!(s.next(), None);
        s.disabled = false;
        s.request(5000);
        assert!(s.start(5000));
    }
}
