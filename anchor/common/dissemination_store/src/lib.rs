//! Handoff store for SIP-94 §6 envelope disseminations.
//!
//! The message receiver writes the first validator-accepted `EnvelopeDissemination` per
//! `(validator, slot)`; the envelope duty runner awaits it. Message validation's first-valid
//! rule delivers at most one dissemination per key for the process lifetime, so the store is
//! first-write-wins and never replaces an entry.

use std::collections::HashMap;

use bls::PublicKeyBytes;
use parking_lot::Mutex;
use ssv_types::dissemination::EnvelopeDissemination;
use tokio::{sync::oneshot, time::Instant};
use types::Slot;

/// Number of slots an entry stays readable, mirroring the decided-block context retention.
const MAX_DISSEMINATION_AGE_SLOTS: u64 = 4;

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct Key {
    validator: PublicKeyBytes,
    slot: Slot,
}

enum Entry {
    /// The accepted dissemination for the key.
    Ready(EnvelopeDissemination),
    /// Runners awaiting the dissemination. Senders are pruned when closed, so timed-out or
    /// cancelled waiters do not accumulate.
    Waiting(Vec<oneshot::Sender<EnvelopeDissemination>>),
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

    /// Records the accepted dissemination for `(validator, slot)` and wakes every waiter.
    ///
    /// First-write-wins: a `Ready` entry is never replaced. Entries older than
    /// `MAX_DISSEMINATION_AGE_SLOTS` relative to the inserted slot are dropped on insert
    /// (addition on the stored side, so an early slot cannot underflow).
    pub fn insert(&self, validator: PublicKeyBytes, dissemination: EnvelopeDissemination) {
        let slot = dissemination.slot;
        let key = Key { validator, slot };
        let mut inner = self.inner.lock();
        Self::sweep(&mut inner, slot);

        match inner.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Entry::Ready(dissemination));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => match entry.get_mut() {
                Entry::Ready(_) => {}
                Entry::Waiting(waiters) => {
                    for waiter in waiters.drain(..) {
                        // A dropped receiver (timed-out waiter) is fine; the value stays Ready.
                        let _ = waiter.send(dissemination.clone());
                    }
                    entry.insert(Entry::Ready(dissemination));
                }
            },
        }
    }

    /// Awaits the dissemination for `(validator, slot)` until `deadline`.
    ///
    /// Returns immediately when the entry is already `Ready`. A timed-out or cancelled wait
    /// leaves a `Ready` value available for a later call. Waiter registrations sweep stale
    /// keys and prune closed senders, so unanswered waits stay bounded even when no
    /// dissemination ever arrives.
    pub async fn wait(
        &self,
        validator: PublicKeyBytes,
        slot: Slot,
        deadline: Instant,
    ) -> Option<EnvelopeDissemination> {
        let receiver = {
            let key = Key { validator, slot };
            let mut inner = self.inner.lock();
            Self::sweep(&mut inner, slot);
            for entry in inner.values_mut() {
                if let Entry::Waiting(waiters) = entry {
                    waiters.retain(|waiter| !waiter.is_closed());
                }
            }

            match inner
                .entry(key)
                .or_insert_with(|| Entry::Waiting(Vec::new()))
            {
                Entry::Ready(dissemination) => return Some(dissemination.clone()),
                Entry::Waiting(waiters) => {
                    let (sender, receiver) = oneshot::channel();
                    waiters.push(sender);
                    receiver
                }
            }
        };

        tokio::time::timeout_at(deadline, receiver).await.ok()?.ok()
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

    fn dissemination(slot: u64) -> EnvelopeDissemination {
        EnvelopeDissemination {
            slot: Slot::new(slot),
            envelope: VariableList::new(vec![0xAA; 8]).unwrap(),
        }
    }

    fn pubkey(byte: u8) -> PublicKeyBytes {
        PublicKeyBytes::deserialize(&[byte; 48]).expect("48 bytes is a valid pubkey length")
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_millis(200)
    }

    #[tokio::test]
    async fn ready_before_wait_returns_immediately() {
        let store = DisseminationStore::new();
        store.insert(pubkey(1), dissemination(5));

        let got = store.wait(pubkey(1), Slot::new(5), soon()).await;
        assert_eq!(got, Some(dissemination(5)));
    }

    #[tokio::test]
    async fn wait_before_ready_wakes_all_same_key_waiters() {
        let store = Arc::new(DisseminationStore::new());
        let deadline = Instant::now() + Duration::from_secs(5);
        let w1 = tokio::spawn({
            let store = store.clone();
            async move { store.wait(pubkey(1), Slot::new(5), deadline).await }
        });
        let w2 = tokio::spawn({
            let store = store.clone();
            async move { store.wait(pubkey(1), Slot::new(5), deadline).await }
        });
        // Let both waiters register before the insert.
        tokio::time::sleep(Duration::from_millis(50)).await;

        store.insert(pubkey(1), dissemination(5));

        assert_eq!(w1.await.unwrap(), Some(dissemination(5)));
        assert_eq!(w2.await.unwrap(), Some(dissemination(5)));
    }

    #[tokio::test]
    async fn timed_out_wait_can_retry_against_ready() {
        let store = DisseminationStore::new();

        let got = store.wait(pubkey(1), Slot::new(5), soon()).await;
        assert_eq!(got, None, "no insert: the wait must time out");

        store.insert(pubkey(1), dissemination(5));
        let got = store.wait(pubkey(1), Slot::new(5), soon()).await;
        assert_eq!(
            got,
            Some(dissemination(5)),
            "the value must stay available after an earlier timed-out wait"
        );
    }

    #[tokio::test]
    async fn first_write_wins() {
        let store = DisseminationStore::new();
        let first = dissemination(5);
        let mut second = dissemination(5);
        second.envelope = VariableList::new(vec![0xBB; 8]).unwrap();

        store.insert(pubkey(1), first.clone());
        store.insert(pubkey(1), second);

        let got = store.wait(pubkey(1), Slot::new(5), soon()).await;
        assert_eq!(got, Some(first), "a Ready entry must never be replaced");
    }

    #[tokio::test]
    async fn keys_are_isolated() {
        let store = DisseminationStore::new();
        store.insert(pubkey(1), dissemination(5));

        assert_eq!(store.wait(pubkey(2), Slot::new(5), soon()).await, None);
        assert_eq!(store.wait(pubkey(1), Slot::new(6), soon()).await, None);
    }

    #[tokio::test]
    async fn insert_evicts_entries_past_the_age_window() {
        let store = DisseminationStore::new();
        store.insert(pubkey(1), dissemination(5));
        store.insert(
            pubkey(1),
            dissemination(5 + MAX_DISSEMINATION_AGE_SLOTS + 1),
        );

        assert_eq!(
            store.wait(pubkey(1), Slot::new(5), soon()).await,
            None,
            "an insert past the age window must evict the older entry"
        );
    }

    #[tokio::test]
    async fn unanswered_waits_stay_bounded() {
        let store = DisseminationStore::new();
        // Many timed-out waits across distinct slots, no inserts at all.
        for slot in 0..100u64 {
            let _ = store.wait(pubkey(1), Slot::new(slot), Instant::now()).await;
        }

        let inner = store.inner.lock();
        assert!(
            inner.len() as u64 <= MAX_DISSEMINATION_AGE_SLOTS + 1,
            "wait-created entries must be swept; got {} keys",
            inner.len()
        );
        let waiters: usize = inner
            .values()
            .map(|entry| match entry {
                Entry::Ready(_) => 0,
                Entry::Waiting(waiters) => waiters.len(),
            })
            .sum();
        // The final wait's own sender has no later registration to prune it; everything
        // older must be gone.
        assert!(
            waiters <= 1,
            "closed senders must be pruned on later registrations; got {waiters}"
        );
    }
}
