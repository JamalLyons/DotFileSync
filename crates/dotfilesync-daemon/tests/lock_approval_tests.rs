//! Resource-lock serialization and the plan/approval contract, using harmless fixtures only.

use dotfilesync_daemon::approval::{
    ApprovalError, ApprovalStore, ApprovalToken, Plan, PlannedAction,
};
use dotfilesync_daemon::locks::{LockMode, ResourceKey, ResourceLocks};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const A: ResourceKey = ResourceKey::Named("fixture-a");
const B: ResourceKey = ResourceKey::Named("fixture-b");

/// Run `n` threads that each take `mode` on `keys` and record peak concurrency inside the
/// critical section (a harmless sleep standing in for a mutation).
fn peak_concurrency(keys: &[ResourceKey], mode: LockMode, n: usize) -> usize {
    let locks = ResourceLocks::new();
    let inside = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(n));
    let handles: Vec<_> = (0..n)
        .map(|i| {
            let (locks, inside, peak, barrier) = (
                Arc::clone(&locks),
                Arc::clone(&inside),
                Arc::clone(&peak),
                Arc::clone(&barrier),
            );
            let key = keys[i % keys.len()].clone();
            std::thread::spawn(move || {
                barrier.wait();
                let _g = locks.acquire(&[(key, mode)], &|| false).unwrap();
                let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(60));
                inside.fetch_sub(1, Ordering::SeqCst);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    peak.load(Ordering::SeqCst)
}

#[test]
fn conflicting_mutations_on_one_resource_are_serialized() {
    assert_eq!(peak_concurrency(&[A], LockMode::Exclusive, 4), 1);
}

#[test]
fn mutations_on_different_resources_run_concurrently() {
    assert_eq!(peak_concurrency(&[A, B], LockMode::Exclusive, 2), 2);
}

#[test]
fn reads_share_a_resource() {
    assert_eq!(peak_concurrency(&[A], LockMode::Shared, 4), 4);
}

#[test]
fn writer_waits_for_readers_and_new_readers_queue_behind_writer() {
    let locks = ResourceLocks::new();
    let reader = locks
        .acquire(&[(A.clone(), LockMode::Shared)], &|| false)
        .unwrap();

    let writer_in = Arc::new(AtomicBool::new(false));
    let w = {
        let (locks, writer_in) = (Arc::clone(&locks), Arc::clone(&writer_in));
        std::thread::spawn(move || {
            let _g = locks
                .acquire(&[(A, LockMode::Exclusive)], &|| false)
                .unwrap();
            writer_in.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
        })
    };
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !writer_in.load(Ordering::SeqCst),
        "writer must wait for the reader"
    );

    // A new reader arriving while the writer waits must not starve it.
    let late_reader_in = Arc::new(AtomicBool::new(false));
    let r2 = {
        let (locks, flag, writer_in) = (
            Arc::clone(&locks),
            Arc::clone(&late_reader_in),
            Arc::clone(&writer_in),
        );
        std::thread::spawn(move || {
            let _g = locks.acquire(&[(A, LockMode::Shared)], &|| false).unwrap();
            assert!(
                writer_in.load(Ordering::SeqCst),
                "late reader overtook the writer"
            );
            flag.store(true, Ordering::SeqCst);
        })
    };
    std::thread::sleep(Duration::from_millis(50));
    assert!(!late_reader_in.load(Ordering::SeqCst));

    // Unrelated resources stay available throughout.
    let t = Instant::now();
    drop(
        locks
            .acquire(&[(B, LockMode::Exclusive)], &|| false)
            .unwrap(),
    );
    assert!(t.elapsed() < Duration::from_millis(50));

    drop(reader);
    w.join().unwrap();
    r2.join().unwrap();
    assert_eq!(locks.holders(&A), (0, false));
}

#[test]
fn waiting_for_a_lock_can_be_cancelled() {
    let locks = ResourceLocks::new();
    let _held = locks
        .acquire(&[(A, LockMode::Exclusive)], &|| false)
        .unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let h = {
        let (locks, cancel) = (Arc::clone(&locks), Arc::clone(&cancel));
        std::thread::spawn(move || {
            locks
                .acquire(&[(A, LockMode::Exclusive)], &|| {
                    cancel.load(Ordering::SeqCst)
                })
                .is_err()
        })
    };
    std::thread::sleep(Duration::from_millis(80));
    cancel.store(true, Ordering::SeqCst);
    assert!(h.join().unwrap(), "cancelled waiter must give up");
    // The cancelled waiter left no residue that would block readers later.
    drop(_held);
    drop(locks.acquire(&[(A, LockMode::Shared)], &|| false).unwrap());
}

#[test]
fn multi_resource_requests_cannot_deadlock() {
    let locks = ResourceLocks::new();
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let locks = Arc::clone(&locks);
            std::thread::spawn(move || {
                // Opposite orders on alternate threads.
                let reqs = if i % 2 == 0 {
                    [(A, LockMode::Exclusive), (B, LockMode::Exclusive)]
                } else {
                    [(B, LockMode::Exclusive), (A, LockMode::Exclusive)]
                };
                for _ in 0..20 {
                    drop(locks.acquire(&reqs, &|| false).unwrap());
                }
            })
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    for h in handles {
        while !h.is_finished() {
            assert!(Instant::now() < deadline, "deadlock");
            std::thread::sleep(Duration::from_millis(10));
        }
        h.join().unwrap();
    }
}

// ---- approvals ---------------------------------------------------------------------------

const OWNER: u32 = 501;
const TTL: u64 = 60_000;

fn plan(resource: &str, state: &str) -> Plan {
    Plan {
        plan_id: "p-1".into(),
        resource: resource.into(),
        actions: vec![PlannedAction {
            action: "fixture.touch".into(),
            target: "fixture://example".into(),
            summary: "harmless fixture step".into(),
            reversible: true,
        }],
        state_fingerprint: state.into(),
        owner_uid: OWNER,
    }
}

#[test]
fn approval_redeems_once_for_the_exact_plan_and_state() {
    let store = ApprovalStore::new();
    let p = plan("fixture-a", "state-1");
    let token = store.issue(&p, OWNER, 1_000, TTL).unwrap();
    assert_eq!(store.redeem(&token, &p, OWNER, "state-1", 2_000), Ok(()));
    // Replay is refused.
    assert_eq!(
        store.redeem(&token, &p, OWNER, "state-1", 2_001),
        Err(ApprovalError::AlreadyUsed)
    );
}

#[test]
fn changed_plan_changed_state_wrong_owner_and_expiry_are_refused() {
    let store = ApprovalStore::new();
    let p = plan("fixture-a", "state-1");

    let t = store.issue(&p, OWNER, 0, TTL).unwrap();
    let mut changed = p.clone();
    changed.actions[0].target = "fixture://other".into();
    assert_eq!(
        store.redeem(&t, &changed, OWNER, "state-1", 1),
        Err(ApprovalError::PlanMismatch)
    );
    // The failed attempt burned the token: correcting the input does not revive it.
    assert_eq!(
        store.redeem(&t, &p, OWNER, "state-1", 2),
        Err(ApprovalError::AlreadyUsed)
    );

    let t = store.issue(&p, OWNER, 0, TTL).unwrap();
    assert_eq!(
        store.redeem(&t, &p, OWNER, "state-2", 1),
        Err(ApprovalError::StateChanged)
    );

    let t = store.issue(&p, OWNER, 0, TTL).unwrap();
    assert_eq!(
        store.redeem(&t, &p, OWNER + 1, "state-1", 1),
        Err(ApprovalError::WrongOwner)
    );

    let t = store.issue(&p, OWNER, 0, TTL).unwrap();
    assert_eq!(
        store.redeem(&t, &p, OWNER, "state-1", TTL),
        Err(ApprovalError::Expired)
    );

    assert_eq!(
        store.issue(&p, OWNER + 1, 0, TTL),
        Err(ApprovalError::WrongOwner)
    );
    assert_eq!(
        store.redeem(
            &ApprovalToken::from_client("forged"),
            &p,
            OWNER,
            "state-1",
            1
        ),
        Err(ApprovalError::UnknownToken)
    );
}

#[test]
fn approving_a_new_plan_invalidates_older_approvals_for_that_resource() {
    let store = ApprovalStore::new();
    let old = plan("fixture-a", "state-1");
    let old_token = store.issue(&old, OWNER, 0, TTL).unwrap();
    let other_resource = store.issue(&plan("fixture-b", "s"), OWNER, 0, TTL).unwrap();

    let new = plan("fixture-a", "state-2");
    let new_token = store.issue(&new, OWNER, 1, TTL).unwrap();
    assert_eq!(
        store.redeem(&old_token, &old, OWNER, "state-1", 2),
        Err(ApprovalError::UnknownToken)
    );
    assert_eq!(store.redeem(&new_token, &new, OWNER, "state-2", 2), Ok(()));
    assert_eq!(
        store.redeem(&other_resource, &plan("fixture-b", "s"), OWNER, "s", 2),
        Ok(())
    );

    let t = store.issue(&new, OWNER, 3, TTL).unwrap();
    store.revoke_resource("fixture-a");
    assert_eq!(
        store.redeem(&t, &new, OWNER, "state-2", 4),
        Err(ApprovalError::UnknownToken)
    );
}

#[test]
fn tokens_never_appear_in_debug_output_and_are_unpredictable() {
    let store = ApprovalStore::new();
    let p = plan("fixture-a", "s");
    let t1 = store.issue(&p, OWNER, 0, TTL).unwrap();
    let t2 = store.issue(&plan("fixture-b", "s"), OWNER, 0, TTL).unwrap();
    assert_eq!(t1.expose().len(), 64);
    assert_ne!(t1.expose(), t2.expose());
    let dbg = format!("{t1:?} {t1}");
    assert!(!dbg.contains(t1.expose()), "{dbg}");
    assert!(!format!("{store:?}").contains(t1.expose()));
}

#[test]
fn plan_digest_covers_every_field() {
    let p = plan("fixture-a", "s");
    let base = p.digest();
    let mut q = p.clone();
    q.owner_uid += 1;
    assert_ne!(q.digest(), base);
    let mut q = p.clone();
    q.actions[0].reversible = false;
    assert_ne!(q.digest(), base);
    let mut q = p.clone();
    q.plan_id = "p-2".into();
    assert_ne!(q.digest(), base);
    assert_eq!(p.clone().digest(), base);
}
