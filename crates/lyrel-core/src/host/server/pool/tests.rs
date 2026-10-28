use super::*;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

#[test]
fn the_pool_reuses_a_warm_worker() {
    let pool = Pool::new();

    let settled = || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let st = pool.inner.state.lock().unwrap();
            if st.idle == st.workers {
                return;
            }
            drop(st);
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the pool never went idle");
    };

    for _ in 0..50 {
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(pool.submit(move || {
            let _ = tx.send(());
        }));
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        settled();
    }
    let st = pool.inner.state.lock().unwrap();
    assert_eq!(
        st.workers, 1,
        "50 sequential jobs should not want 50 threads"
    );
}

#[test]
fn a_worker_is_spawned_when_the_backlog_outruns_the_parked_workers() {
    let state = |jobs: usize, workers: usize, idle: usize| PoolState {
        jobs: (0..jobs).map(|_| Box::new(|| ()) as Job).collect(),
        workers,
        idle,
        closed: false,
    };

    assert!(!state(1, 1, 1).wants_another_worker());

    assert!(state(2, 1, 1).wants_another_worker());

    assert!(state(1, 1, 0).wants_another_worker());

    assert!(!state(64, MAX_WORKERS, 0).wants_another_worker());
}

#[test]
fn the_pool_grows_for_concurrent_work() {
    let pool = Pool::new();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    for _ in 0..8 {
        let gate = Arc::clone(&gate);
        let done_tx = done_tx.clone();
        assert!(pool.submit(move || {
            let (lock, cv) = &*gate;
            let mut open = lock.lock().unwrap();
            let _ = done_tx.send(());
            while !*open {
                open = cv.wait(open).unwrap();
            }
        }));
    }
    for _ in 0..8 {
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    assert_eq!(pool.inner.state.lock().unwrap().workers, 8);

    let (lock, cv) = &*gate;
    *lock.lock().unwrap() = true;
    cv.notify_all();
    pool.close();
}

#[test]
fn closing_the_pool_drops_queued_work() {
    let pool = Pool::new();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let ran = Arc::new(AtomicBool::new(false));

    let blocker = Arc::clone(&gate);
    assert!(pool.submit(move || {
        let (lock, cv) = &*blocker;
        let mut open = lock.lock().unwrap();
        while !*open {
            open = cv.wait(open).unwrap();
        }
    }));
    std::thread::sleep(Duration::from_millis(100));
    let ran2 = Arc::clone(&ran);
    pool.submit(move || ran2.store(true, Ordering::SeqCst));

    pool.close();
    let (lock, cv) = &*gate;
    *lock.lock().unwrap() = true;
    cv.notify_all();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !ran.load(Ordering::SeqCst),
        "a job queued at close still ran"
    );
    assert!(!pool.submit(|| {}), "a closed pool must refuse work");
}
