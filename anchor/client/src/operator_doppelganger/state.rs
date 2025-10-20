use std::collections::HashMap;

use ssv_types::CommitteeId;

/// Operating mode for doppelgänger protection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoppelgangerMode {
    /// Monitor mode: listen for messages with our operator ID
    Monitor,
    /// Active mode: normal operation
    Active,
}

/// State for operator doppelgänger detection
#[derive(Debug, Clone)]
pub struct DoppelgangerState {
    /// Current operating mode
    mode: DoppelgangerMode,
    /// Maximum consensus height observed per committee
    recent_max_height: HashMap<CommitteeId, u64>,
    /// Freshness threshold (K) - messages within this many heights are considered fresh
    fresh_k: u64,
}

impl DoppelgangerState {
    /// Create a new doppelgänger state in monitor mode
    pub fn new(fresh_k: u64) -> Self {
        Self {
            mode: DoppelgangerMode::Monitor,
            recent_max_height: HashMap::new(),
            fresh_k,
        }
    }

    /// Get the current mode
    #[cfg(test)]
    #[must_use]
    pub fn mode(&self) -> DoppelgangerMode {
        self.mode
    }

    /// Check if still in monitor mode
    #[must_use]
    pub fn is_monitoring(&self) -> bool {
        matches!(self.mode, DoppelgangerMode::Monitor)
    }

    /// Explicitly transition to active mode
    ///
    /// This should be called by the service when the monitoring period ends.
    pub fn set_active(&mut self) {
        self.mode = DoppelgangerMode::Active;
    }

    /// Update the maximum height for a committee
    fn update_max_height(&mut self, committee: CommitteeId, height: u64) {
        self.recent_max_height
            .entry(committee)
            .and_modify(|h| *h = (*h).max(height))
            .or_insert(height);
    }

    /// Check if a message height is considered "fresh" for twin detection
    ///
    /// A message is fresh if: height >= (recent_max_height - K)
    #[must_use]
    fn is_fresh(&self, committee: CommitteeId, height: u64) -> bool {
        if let Some(&max_height) = self.recent_max_height.get(&committee) {
            let baseline = max_height.saturating_sub(self.fresh_k);
            height >= baseline
        } else {
            // If we haven't seen any messages for this committee, consider it fresh
            true
        }
    }

    /// Update max height for a committee and return if the height is fresh
    ///
    /// This is an atomic operation that updates the height tracking and
    /// determines freshness in one call, useful for twin detection.
    #[must_use]
    pub fn update_and_check_freshness(&mut self, committee: CommitteeId, height: u64) -> bool {
        self.update_max_height(committee, height);
        self.is_fresh(committee, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let state = DoppelgangerState::new(3);
        assert_eq!(state.mode(), DoppelgangerMode::Monitor);
        assert!(state.is_monitoring());
    }

    #[test]
    fn test_mode_transition() {
        let mut state = DoppelgangerState::new(3);

        // Initially monitoring
        assert_eq!(state.mode(), DoppelgangerMode::Monitor);
        assert!(state.is_monitoring());

        // Explicitly transition to active
        state.set_active();
        assert_eq!(state.mode(), DoppelgangerMode::Active);
        assert!(!state.is_monitoring());
    }

    #[test]
    fn test_height_tracking() {
        let mut state = DoppelgangerState::new(3);
        let committee = CommitteeId([1u8; 32]);

        state.update_max_height(committee, 10);
        assert_eq!(state.recent_max_height.get(&committee), Some(&10));

        // Update with lower height - should not change
        state.update_max_height(committee, 5);
        assert_eq!(state.recent_max_height.get(&committee), Some(&10));

        // Update with higher height
        state.update_max_height(committee, 15);
        assert_eq!(state.recent_max_height.get(&committee), Some(&15));
    }

    #[test]
    fn test_freshness_check() {
        let mut state = DoppelgangerState::new(3);
        let committee = CommitteeId([1u8; 32]);

        // No messages seen yet - everything is fresh
        assert!(state.is_fresh(committee, 0));
        assert!(state.is_fresh(committee, 100));

        // Set max height to 10
        state.update_max_height(committee, 10);

        // Fresh range: [10 - 3, 10] = [7, 10]
        assert!(!state.is_fresh(committee, 6)); // Below baseline
        assert!(state.is_fresh(committee, 7)); // At baseline
        assert!(state.is_fresh(committee, 10)); // At max
        assert!(state.is_fresh(committee, 11)); // Above max (still fresh)
    }

    #[test]
    fn test_freshness_with_small_height() {
        let mut state = DoppelgangerState::new(3);
        let committee = CommitteeId([1u8; 32]);

        // Set max height to 2 (less than K)
        state.update_max_height(committee, 2);

        // baseline = max(0, 2 - 3) = 0
        assert!(state.is_fresh(committee, 0));
        assert!(state.is_fresh(committee, 1));
        assert!(state.is_fresh(committee, 2));
    }
}
