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
    /// Whether we're still in the startup grace period
    ///
    /// ## Why we need a grace period
    ///
    /// Gossipsub stores messages in a sliding window cache (mcache) for gossip propagation.
    /// Messages remain in this cache for `history_length × heartbeat_interval` (~4.2s in Anchor).
    ///
    /// **The restart vulnerability:**
    /// 1. Node sends messages (QBFT + partial signatures) at t=0
    /// 2. Messages propagate to peers' mcache
    /// 3. Node crashes/restarts at t=2s
    /// 4. Gossipsub seen_cache is cleared (not persisted)
    /// 5. Messages still in peers' mcache (~2s remaining)
    /// 6. Node reconnects and receives its own messages via IHAVE/IWANT
    /// 7. FALSE POSITIVE: Messages have our operator ID but we think they're from a twin!
    ///
    /// **Solution:**
    /// Wait for gossip cache expiry (~5s) before checking messages. This ensures our own
    /// old messages have expired from the network before we start detecting twins.
    ///
    /// Set to `true` initially, then set to `false` by the monitor task after sleeping
    /// for the grace period duration.
    in_grace_period: bool,
}

impl Default for DoppelgangerState {
    fn default() -> Self {
        Self::new()
    }
}

impl DoppelgangerState {
    /// Create a new doppelgänger state in monitor mode with grace period active
    pub fn new() -> Self {
        Self {
            mode: DoppelgangerMode::Monitor,
            in_grace_period: true,
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

    /// Mark the startup grace period as complete
    ///
    /// This should be called by the monitor task after sleeping for the grace period duration.
    /// After this is called, `check_message()` will start detecting twins.
    pub fn end_grace_period(&mut self) {
        self.in_grace_period = false;
    }

    /// Check if we're still in the startup grace period
    #[must_use]
    pub fn is_in_grace_period(&self) -> bool {
        self.in_grace_period
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_transition() {
        let mut state = DoppelgangerState::new();

        // Initially monitoring
        assert_eq!(state.mode(), DoppelgangerMode::Monitor);
        assert!(state.is_monitoring());

        // Explicitly transition to active
        state.set_active();
        assert_eq!(state.mode(), DoppelgangerMode::Active);
        assert!(!state.is_monitoring());
    }

    #[test]
    fn test_grace_period_initially_active() {
        let state = DoppelgangerState::new();

        // Initially in grace period
        assert!(state.is_in_grace_period());
    }

    #[test]
    fn test_grace_period_can_be_ended() {
        let mut state = DoppelgangerState::new();

        // Initially in grace period
        assert!(state.is_in_grace_period());

        // End grace period
        state.end_grace_period();

        // Should no longer be in grace period
        assert!(!state.is_in_grace_period());
    }
}
