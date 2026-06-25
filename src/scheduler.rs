//! Debounce + coalesce scheduler for non-blocking diagnostics.
//!
//! Rapid `did_change` events (one per keystroke) would otherwise queue many
//! slow `compile-file` operations on the single Guile REPL, starving
//! interactive requests (completion/hover). The scheduler collapses a burst of
//! schedules for the same key into a single run of the *latest* task, after a
//! short quiet period.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct DebouncedScheduler<K: Eq + Hash + Clone + Send + 'static> {
    /// Per-key generation counter; only the latest-scheduled task runs.
    gen: Arc<Mutex<HashMap<K, u64>>>,
}

impl<K: Eq + Hash + Clone + Send + 'static> DebouncedScheduler<K> {
    pub fn new() -> Self {
        Self {
            gen: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Schedule `run(key)` to fire after `debounce` of quiet. If `schedule` is
    /// called again for the same key before the debounce elapses, only the most
    /// recent call's `run` executes; earlier ones are dropped.
    pub fn schedule<F, Fut>(&self, key: K, debounce: Duration, run: F)
    where
        F: FnOnce(K) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        // Bump this key's generation; only the latest-scheduled task survives.
        let my_gen = {
            let mut g = self.gen.lock().expect("gen map poisoned");
            let entry = g.entry(key.clone()).or_insert(0);
            *entry += 1;
            *entry
        };
        let gen = self.gen.clone();
        tokio::spawn(async move {
            tokio::time::sleep(debounce).await;
            // If a newer schedule arrived for this key, drop this stale task.
            let current = gen
                .lock()
                .expect("gen map poisoned")
                .get(&key)
                .copied()
                .unwrap_or(0);
            if current != my_gen {
                return;
            }
            run(key).await;
        });
    }
}

impl<K: Eq + Hash + Clone + Send + 'static> Default for DebouncedScheduler<K> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test(start_paused = true)]
    async fn single_schedule_runs_once_after_debounce() {
        let s = DebouncedScheduler::<String>::new();
        let count = Arc::new(AtomicU32::new(0));
        let c = count.clone();
        s.schedule(
            "a".to_string(),
            Duration::from_millis(100),
            move |_| async move {
                c.fetch_add(1, Ordering::SeqCst);
            },
        );

        // Before the debounce elapses -> nothing has run.
        tokio::time::advance(Duration::from_millis(50)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 0);

        // Past the debounce -> the task ran exactly once.
        tokio::time::advance(Duration::from_millis(100)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn rapid_burst_coalesces_into_single_run() {
        // Five schedules for the same key within the debounce window must
        // collapse to exactly one run of the latest task.
        let s = DebouncedScheduler::<String>::new();
        let count = Arc::new(AtomicU32::new(0));
        for _ in 0..5 {
            let c = count.clone();
            s.schedule(
                "a".to_string(),
                Duration::from_millis(300),
                move |_| async move {
                    c.fetch_add(1, Ordering::SeqCst);
                },
            );
            tokio::time::advance(Duration::from_millis(50)).await;
        }
        // Still within debounce of the last schedule -> nothing yet.
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 0);

        // Past the debounce of the latest schedule -> exactly one run.
        tokio::time::advance(Duration::from_millis(400)).await;
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn independent_keys_run_independently() {
        let s = DebouncedScheduler::<String>::new();
        let count = Arc::new(AtomicU32::new(0));
        for key in ["a", "b"] {
            let c = count.clone();
            s.schedule(
                key.to_string(),
                Duration::from_millis(100),
                move |_| async move {
                    c.fetch_add(1, Ordering::SeqCst);
                },
            );
        }
        // `sleep` on a paused runtime auto-advances time and lets all due
        // timers fire and their tasks run.
        tokio::time::sleep(Duration::from_millis(250)).await;
        // Two different keys -> two runs (no cross-coalescing).
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
}
