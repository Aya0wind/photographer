//! Independent FIFO budgets for image, semantic and face indexing. A permit is
//! held from task claim through persistence, so `running` cannot exceed the cap.
use std::sync::{Arc, Condvar, Mutex, OnceLock};

pub enum Domain {
    Image,
    Semantic,
    Face,
}

pub fn budget(domain: Domain) -> Arc<Budget> {
    static IMAGE: OnceLock<Arc<Budget>> = OnceLock::new();
    static SEMANTIC: OnceLock<Arc<Budget>> = OnceLock::new();
    static FACE: OnceLock<Arc<Budget>> = OnceLock::new();
    let slot = match domain {
        Domain::Image => &IMAGE,
        Domain::Semantic => &SEMANTIC,
        Domain::Face => &FACE,
    };
    slot.get_or_init(|| Arc::new(Budget::dynamic())).clone()
}

#[derive(Default)]
struct State {
    next: u64,
    serving: u64,
    used: usize,
}

pub struct Budget {
    limit: usize,
    dynamic: bool,
    state: Mutex<State>,
    ready: Condvar,
}
pub struct Permit {
    budget: Arc<Budget>,
    pub count: usize,
}

impl Budget {
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            dynamic: false,
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        }
    }
    fn dynamic() -> Self {
        Self {
            dynamic: true,
            ..Self::new(1)
        }
    }
    fn limit(&self) -> usize {
        if self.dynamic {
            super::index_parallelism()
        } else {
            self.limit
        }
    }
    pub fn acquire(self: &Arc<Self>, requested: usize) -> Permit {
        let mut state = self.state.lock().expect("index budget poisoned");
        let ticket = state.next;
        state.next += 1;
        let count = loop {
            let limit = self.limit();
            let count = requested.max(1).min(limit);
            if ticket == state.serving && state.used + count <= limit {
                break count;
            }
            state = self.ready.wait(state).expect("index budget poisoned");
        };
        state.used += count;
        state.serving += 1;
        drop(state);
        self.ready.notify_all();
        Permit {
            budget: self.clone(),
            count,
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.budget
            .state
            .lock()
            .expect("index budget poisoned")
            .used -= self.count;
        self.budget.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn concurrent_claims_never_exceed_budget_and_permits_are_released() {
        let gate = Arc::new(Budget::new(2));
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let gate = gate.clone();
                let active = &active;
                let peak = &peak;
                scope.spawn(move || {
                    let _permit = gate.acquire(1);
                    let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(gate.acquire(16).count, 2);
    }
    #[test]
    fn indexing_domains_have_independent_budgets() {
        assert!(!Arc::ptr_eq(
            &budget(Domain::Image),
            &budget(Domain::Semantic)
        ));
        assert!(!Arc::ptr_eq(&budget(Domain::Image), &budget(Domain::Face)));
        assert_eq!(
            budget(Domain::Image).limit(),
            super::super::index_parallelism()
        );
    }
    #[test]
    fn concurrency_is_calculated_from_each_environment_not_a_fixed_number() {
        for (cores, available, want) in [
            (1, 1, 1),
            (4, 8, 2),
            (12, 24, 6),
            (16, 32, 8),
            (32, 64, 16),
            (12, 4, 2),
        ] {
            assert_eq!(super::super::index_parallelism_for(cores, available), want);
        }
    }
}
