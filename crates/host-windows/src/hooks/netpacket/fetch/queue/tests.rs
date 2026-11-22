//! The queue's own tests: the bound, the dedup and the counters.

use super::*;
use crate::hooks::netpacket::decision::RequestTarget;

fn request(gid: u64) -> Request {
    Request {
        app: 480,
        target: RequestTarget { depot: 731, gid },
    }
}

#[test]
fn a_closed_queue_accepts_nothing_and_counts_the_reason() {
    let queue = Queue::new();
    assert_eq!(
        queue.enqueue(request(1)),
        Enqueued::Dropped(DropReason::Inactive)
    );
    assert_eq!(queue.pending(), 0);
    assert_eq!(queue.counters().dropped_inactive, 1);
    assert!(!queue.open_now());
}

#[test]
fn an_open_queue_hands_targets_over_oldest_first() {
    let queue = Queue::new();
    queue.open();
    assert_eq!(queue.enqueue(request(1)), Enqueued::Queued);
    assert_eq!(queue.enqueue(request(2)), Enqueued::Queued);
    assert_eq!(queue.pending(), 2);
    let wait = Duration::from_secs(0);
    assert_eq!(queue.next(wait).map(|r| r.target.gid), Some(1));
    assert_eq!(queue.next(wait).map(|r| r.target.gid), Some(2));
    assert_eq!(queue.next(wait), None, "the ring is drained, not closed");
    assert_eq!(queue.counters().queued, 2);
}

#[test]
fn the_ring_is_bounded_and_evicts_the_oldest_with_a_count() {
    let queue = Queue::new();
    queue.open();
    for gid in 0..CAPACITY as u64 {
        assert_eq!(queue.enqueue(request(gid)), Enqueued::Queued);
    }
    assert_eq!(queue.pending(), CAPACITY);
    assert_eq!(
        queue.enqueue(request(9_999)),
        Enqueued::QueuedEvictingOldest
    );
    assert_eq!(queue.pending(), CAPACITY, "the ring never grows past its bound");
    assert_eq!(queue.counters().evicted_oldest, 1);
    assert_eq!(queue.counters().queued, CAPACITY as u64 + 1);
    let wait = Duration::from_secs(0);
    // gid 0 was the oldest and is gone; gid 1 is now the head.
    assert_eq!(queue.next(wait).map(|r| r.target.gid), Some(1));
}

#[test]
fn one_target_is_queued_once_while_it_waits() {
    let queue = Queue::new();
    queue.open();
    assert_eq!(queue.enqueue(request(1)), Enqueued::Queued);
    assert_eq!(
        queue.enqueue(request(1)),
        Enqueued::Dropped(DropReason::AlreadyQueued)
    );
    assert_eq!(queue.pending(), 1);
    assert_eq!(queue.counters().dropped_already_queued, 1);
}

#[test]
fn one_target_is_not_queued_again_while_it_is_in_flight() {
    let queue = Queue::new();
    queue.open();
    queue.enqueue(request(1));
    assert_eq!(queue.next(Duration::ZERO).map(|r| r.target.gid), Some(1));
    // In the worker's hands: a second dispatch of the same message must not
    // start a second round trip.
    assert_eq!(
        queue.enqueue(request(1)),
        Enqueued::Dropped(DropReason::InFlight)
    );
    assert_eq!(queue.counters().dropped_in_flight, 1);
    // Finishing the target lets it be asked for again.
    queue.finish();
    assert_eq!(queue.enqueue(request(1)), Enqueued::Queued);
}

#[test]
fn different_targets_are_never_deduplicated() {
    let queue = Queue::new();
    queue.open();
    assert_eq!(queue.enqueue(request(1)), Enqueued::Queued);
    // Same manifest, different depot: a different code.
    let mut other = request(1);
    other.target.depot = 732;
    assert_eq!(queue.enqueue(other), Enqueued::Queued);
    assert_eq!(queue.pending(), 2);
}

#[test]
fn closing_the_queue_wakes_the_worker_and_refuses_further_work() {
    let queue = Queue::new();
    queue.open();
    queue.close();
    assert_eq!(queue.next(Duration::from_millis(10)), None);
    assert_eq!(
        queue.enqueue(request(1)),
        Enqueued::Dropped(DropReason::Inactive)
    );
}

#[test]
fn counters_saturate_instead_of_wrapping() {
    let queue = Queue::new();
    {
        let mut inner = queue.lock();
        inner.counters.served = u64::MAX;
    }
    queue.record(Tally::Served);
    assert_eq!(queue.counters().served, u64::MAX);
}
