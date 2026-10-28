//! Bounded FIFO scheduling with warm workers and explicit connection retirement.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

use super::{MAX_QUEUED, MAX_WORKERS, WORKER_LINGER};

type Job = Box<dyn FnOnce() + Send + 'static>;

pub(super) struct Pool {
    inner: Arc<PoolInner>,
}

struct PoolInner {
    state: Mutex<PoolState>,
    wake: Condvar,
}

struct PoolState {
    jobs: VecDeque<Job>,
    workers: usize,
    idle: usize,
    closed: bool,
}

impl PoolState {
    fn wants_another_worker(&self) -> bool {
        self.jobs.len() > self.idle && self.workers < MAX_WORKERS
    }
}

impl Pool {
    pub(super) fn new() -> Pool {
        Pool {
            inner: Arc::new(PoolInner {
                state: Mutex::new(PoolState {
                    jobs: VecDeque::new(),
                    workers: 0,
                    idle: 0,
                    closed: false,
                }),
                wake: Condvar::new(),
            }),
        }
    }

    pub(super) fn submit(&self, job: impl FnOnce() + Send + 'static) -> bool {
        let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.closed || st.jobs.len() >= MAX_QUEUED {
            return false;
        }
        st.jobs.push_back(Box::new(job));

        if st.wants_another_worker() {
            st.workers += 1;
            let inner = Arc::clone(&self.inner);
            match std::thread::Builder::new()
                .name("lyrel-control-worker".into())
                .spawn(move || worker(inner))
            {
                Ok(_) => return true,
                Err(e) => {
                    st.workers -= 1;
                    log::warn!("could not start a control worker: {e}");
                    if st.workers == 0 {
                        st.jobs.pop_back();
                        return false;
                    }
                }
            }
        }
        drop(st);
        self.inner.wake.notify_one();
        true
    }

    pub(super) fn close(&self) {
        {
            let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            st.closed = true;
            st.jobs.clear();
        }
        self.inner.wake.notify_all();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.close();
    }
}

fn worker(inner: Arc<PoolInner>) {
    loop {
        let job = {
            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(job) = st.jobs.pop_front() {
                    break job;
                }
                if st.closed {
                    st.workers -= 1;
                    return;
                }
                st.idle += 1;
                let (guard, timeout) = inner
                    .wake
                    .wait_timeout(st, WORKER_LINGER)
                    .unwrap_or_else(|e| e.into_inner());
                st = guard;
                st.idle -= 1;
                if timeout.timed_out() && st.jobs.is_empty() {
                    st.workers -= 1;
                    return;
                }
            }
        };
        job();
    }
}

#[cfg(test)]
mod tests;
