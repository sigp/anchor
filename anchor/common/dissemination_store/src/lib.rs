//! Handoff store for SIP-94 §6 envelope disseminations.
//!
//! The message receiver writes every validator-accepted `EnvelopeDissemination` for a
//! `(validator, slot)`; the envelope duty runner scans them in arrival order for the first
//! whose envelope binds to its own §4 decision. Message validation admits at most one
//! dissemination per (`MessageId`, signer, slot) (SIP-94 §7) and only from committee members,
//! so the candidate list is bounded by committee size on the validated path. The store itself
//! enforces neither: it is a handoff, and validation is its only production writer.

use std::collections::HashMap;

use bls::PublicKeyBytes;
use parking_lot::Mutex;
use ssv_types::{OperatorId, dissemination::EnvelopeDissemination};
use tokio::{sync::watch, time::Instant};
use types::Slot;

/// Number of slots an entry stays readable, mirroring the decided-block context retention.
const MAX_DISSEMINATION_AGE_SLOTS: u64 = 4;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct Key {
    validator: PublicKeyBytes,
    slot: Slot,
}

/// The candidates accepted for one key, and a counter that wakes waiters when one lands.
struct Entry {
    /// Accepted disseminations in arrival order, tagged with the operator that signed each.
    candidates: Vec<(OperatorId, EnvelopeDissemination)>,
    /// Set to `candidates.len()` on every push. A `watch` receiver treats the value present
    /// when it subscribes as already seen, so a waiter must subscribe while holding the same
    /// lock it reads `candidates` under. Subscribing after releasing the lock would miss a
    /// candidate pushed in between and could sleep to its deadline with a match already in
    /// the vector.
    version: watch::Sender<usize>,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            candidates: Vec::new(),
            version: watch::Sender::new(0),
        }
    }
}

/// Shared store connecting the message receiver (writer) to the envelope duty runner (reader).
#[derive(Default)]
pub struct DisseminationStore {
    inner: Mutex<HashMap<Key, Entry>>,
}

impl DisseminationStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an accepted dissemination for `(validator, slot)` and wakes the waiters.
    ///
    /// Every accepted candidate is kept, because the runner selects by content rather than by
    /// arrival: discarding here would hand a Byzantine committee member the slot by letting it
    /// win the race. Entries older than `MAX_DISSEMINATION_AGE_SLOTS` relative to the inserted
    /// slot are dropped on insert (addition on the stored side, so an early slot cannot
    /// underflow).
    pub fn insert(
        &self,
        validator: PublicKeyBytes,
        signer: OperatorId,
        dissemination: EnvelopeDissemination,
    ) {
        let slot = dissemination.slot;
        let mut inner = self.inner.lock();
        Self::sweep(&mut inner, slot);

        let entry = inner.entry(Key { validator, slot }).or_default();
        entry.candidates.push((signer, dissemination));
        entry.version.send_replace(entry.candidates.len());
    }

    /// Returns the first candidate for `(validator, slot)` that `predicate` accepts, waiting
    /// for later arrivals until `deadline`.
    ///
    /// Candidates are visited once each, in arrival order, starting from those already stored.
    /// `predicate` runs outside the store lock, so it may decode and validate the envelope.
    /// Returns `None` when the deadline passes with no candidate accepted.
    pub async fn wait_matching<T>(
        &self,
        validator: PublicKeyBytes,
        slot: Slot,
        deadline: Instant,
        mut predicate: impl FnMut(OperatorId, &EnvelopeDissemination) -> Option<T>,
    ) -> Option<T> {
        let key = Key { validator, slot };
        let mut cursor = 0;

        loop {
            // One critical section for both, per the `version` note above.
            let (candidate, mut version) = {
                let mut inner = self.inner.lock();
                Self::sweep(&mut inner, slot);
                let entry = inner.entry(key).or_default();
                (
                    entry.candidates.get(cursor).cloned(),
                    entry.version.subscribe(),
                )
            };

            if let Some((signer, dissemination)) = candidate {
                cursor += 1;
                if let Some(accepted) = predicate(signer, &dissemination) {
                    return Some(accepted);
                }
                continue;
            }

            match tokio::time::timeout_at(deadline, version.changed()).await {
                // A candidate landed; read it on the next pass.
                Ok(Ok(())) => {}
                // The entry was swept while waiting, so nothing more can arrive under it.
                Ok(Err(_sender_dropped)) => return None,
                Err(_elapsed) => return None,
            }
        }
    }

    /// Drops entries older than `MAX_DISSEMINATION_AGE_SLOTS` relative to `slot`. A call for
    /// an old slot cannot evict a newer entry.
    fn sweep(inner: &mut HashMap<Key, Entry>, slot: Slot) {
        inner.retain(|stored_key, _| stored_key.slot + MAX_DISSEMINATION_AGE_SLOTS >= slot);
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use ssv_types::VariableList;

    use super::*;

    /// A dissemination for `slot` whose envelope bytes are all `tag`, so tests can tell
    /// candidates apart and match on one.
    fn tagged(slot: u64, tag: u8) -> EnvelopeDissemination {
        EnvelopeDissemination {
            slot: Slot::new(slot),
            envelope: VariableList::new(vec![tag; 8]).unwrap(),
        }
    }

    fn dissemination(slot: u64) -> EnvelopeDissemination {
        tagged(slot, 0xAA)
    }

    fn pubkey(byte: u8) -> PublicKeyBytes {
        PublicKeyBytes::deserialize(&[byte; 48]).expect("48 bytes is a valid pubkey length")
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_millis(200)
    }

    /// Accepts any candidate, returning it.
    fn accept_any(
        _signer: OperatorId,
        dissemination: &EnvelopeDissemination,
    ) -> Option<EnvelopeDissemination> {
        Some(dissemination.clone())
    }

    /// Accepts only the candidate whose envelope bytes carry `tag`, standing in for the
    /// runner's decision-binding check.
    fn accept_tag(
        tag: u8,
    ) -> impl FnMut(OperatorId, &EnvelopeDissemination) -> Option<EnvelopeDissemination> {
        move |_signer, dissemination| {
            (dissemination.envelope[0] == tag).then(|| dissemination.clone())
        }
    }

    #[tokio::test]
    async fn stored_candidate_matches_without_waiting() {
        let store = DisseminationStore::new();
        store.insert(pubkey(1), OperatorId(1), dissemination(5));

        let got = store
            .wait_matching(pubkey(1), Slot::new(5), soon(), accept_any)
            .await;
        assert_eq!(got, Some(dissemination(5)));
    }

    #[tokio::test]
    async fn candidates_accumulate_in_arrival_order() {
        let store = DisseminationStore::new();
        store.insert(pubkey(1), OperatorId(1), tagged(5, 0xAA));
        store.insert(pubkey(1), OperatorId(2), tagged(5, 0xBB));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let got = store
            .wait_matching(pubkey(1), Slot::new(5), soon(), move |signer, d| {
                recorder.lock().push((signer, d.envelope[0]));
                None::<()>
            })
            .await;

        assert_eq!(got, None, "no candidate was accepted");
        assert_eq!(
            *seen.lock(),
            vec![(OperatorId(1), 0xAA), (OperatorId(2), 0xBB)],
            "candidates must be visited in arrival order, each tagged with its signer"
        );
    }

    /// The behaviour this store exists for: a rejected candidate must not consume the slot.
    #[tokio::test]
    async fn rejected_candidate_is_skipped_and_a_later_arrival_matches() {
        let store = Arc::new(DisseminationStore::new());
        // The undesired candidate is already stored when the wait starts.
        store.insert(pubkey(1), OperatorId(1), tagged(5, 0xAA));

        let waiter = tokio::spawn({
            let store = store.clone();
            let deadline = Instant::now() + Duration::from_secs(5);
            async move {
                store
                    .wait_matching(pubkey(1), Slot::new(5), deadline, accept_tag(0xBB))
                    .await
            }
        });
        // Let the waiter drain the backlog and register for the next arrival.
        tokio::time::sleep(Duration::from_millis(50)).await;

        store.insert(pubkey(1), OperatorId(2), tagged(5, 0xBB));

        assert_eq!(
            waiter.await.unwrap(),
            Some(tagged(5, 0xBB)),
            "the wait must skip the rejected candidate and take the later matching one"
        );
    }

    #[tokio::test]
    async fn each_candidate_is_visited_once() {
        let store = Arc::new(DisseminationStore::new());
        let visits = Arc::new(Mutex::new(Vec::new()));

        let waiter = tokio::spawn({
            let store = store.clone();
            let recorder = visits.clone();
            let deadline = Instant::now() + Duration::from_millis(400);
            async move {
                store
                    .wait_matching(pubkey(1), Slot::new(5), deadline, move |signer, _| {
                        recorder.lock().push(signer);
                        None::<()>
                    })
                    .await
            }
        });

        for id in 1..=3u64 {
            tokio::time::sleep(Duration::from_millis(30)).await;
            store.insert(pubkey(1), OperatorId(id), dissemination(5));
        }

        assert_eq!(waiter.await.unwrap(), None);
        assert_eq!(
            *visits.lock(),
            vec![OperatorId(1), OperatorId(2), OperatorId(3)],
            "the cursor must not re-offer a candidate the predicate already rejected"
        );
    }

    #[tokio::test]
    async fn wait_times_out_when_nothing_matches() {
        let store = DisseminationStore::new();
        store.insert(pubkey(1), OperatorId(1), tagged(5, 0xAA));

        let got = store
            .wait_matching(pubkey(1), Slot::new(5), soon(), accept_tag(0xBB))
            .await;
        assert_eq!(got, None, "an unmatched backlog must still time out");
    }

    #[tokio::test]
    async fn concurrent_waiters_each_see_the_candidate() {
        let store = Arc::new(DisseminationStore::new());
        let deadline = Instant::now() + Duration::from_secs(5);
        let spawn_waiter = || {
            tokio::spawn({
                let store = store.clone();
                async move {
                    store
                        .wait_matching(pubkey(1), Slot::new(5), deadline, accept_any)
                        .await
                }
            })
        };
        let (w1, w2) = (spawn_waiter(), spawn_waiter());
        // Let both waiters register before the insert.
        tokio::time::sleep(Duration::from_millis(50)).await;

        store.insert(pubkey(1), OperatorId(1), dissemination(5));

        assert_eq!(w1.await.unwrap(), Some(dissemination(5)));
        assert_eq!(w2.await.unwrap(), Some(dissemination(5)));
    }

    #[tokio::test]
    async fn timed_out_wait_can_retry_against_the_backlog() {
        let store = DisseminationStore::new();

        let got = store
            .wait_matching(pubkey(1), Slot::new(5), soon(), accept_any)
            .await;
        assert_eq!(got, None, "no insert: the wait must time out");

        store.insert(pubkey(1), OperatorId(1), dissemination(5));
        let got = store
            .wait_matching(pubkey(1), Slot::new(5), soon(), accept_any)
            .await;
        assert_eq!(
            got,
            Some(dissemination(5)),
            "candidates must stay available after an earlier timed-out wait"
        );
    }

    #[tokio::test]
    async fn keys_are_isolated() {
        let store = DisseminationStore::new();
        store.insert(pubkey(1), OperatorId(1), dissemination(5));

        assert_eq!(
            store
                .wait_matching(pubkey(2), Slot::new(5), soon(), accept_any)
                .await,
            None
        );
        assert_eq!(
            store
                .wait_matching(pubkey(1), Slot::new(6), soon(), accept_any)
                .await,
            None
        );
    }

    #[tokio::test]
    async fn insert_evicts_entries_past_the_age_window() {
        let store = DisseminationStore::new();
        store.insert(pubkey(1), OperatorId(1), dissemination(5));
        store.insert(
            pubkey(1),
            OperatorId(1),
            dissemination(5 + MAX_DISSEMINATION_AGE_SLOTS + 1),
        );

        assert_eq!(
            store
                .wait_matching(pubkey(1), Slot::new(5), soon(), accept_any)
                .await,
            None,
            "an insert past the age window must evict the older entry"
        );
    }

    #[tokio::test]
    async fn unanswered_waits_stay_bounded() {
        let store = DisseminationStore::new();
        // Many timed-out waits across distinct slots, no inserts at all.
        for slot in 0..100u64 {
            let _ = store
                .wait_matching(pubkey(1), Slot::new(slot), Instant::now(), accept_any)
                .await;
        }

        let inner = store.inner.lock();
        assert!(
            inner.len() as u64 <= MAX_DISSEMINATION_AGE_SLOTS + 1,
            "wait-created entries must be swept; got {} keys",
            inner.len()
        );
        assert!(
            inner.values().all(|entry| entry.candidates.is_empty()),
            "a wait must not create candidates"
        );
    }
}
