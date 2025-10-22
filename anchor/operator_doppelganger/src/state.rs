/// State of operator doppelgänger detection
///
/// ## State Transitions
///
/// ```text
/// GracePeriod → Monitoring → Completed
/// ```
///
/// - **GracePeriod**: Waiting for network message caches to expire before checking
/// - **Monitoring**: Actively checking messages for doppelgängers
/// - **Completed**: Monitoring period finished, no longer checking
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoppelgangerState {
    /// In startup grace period - not yet checking for doppelgängers
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
    GracePeriod,

    /// Actively monitoring for doppelgängers - checking all messages
    Monitoring,

    /// Monitoring period completed - no longer checking for doppelgängers
    Completed,
}

impl Default for DoppelgangerState {
    fn default() -> Self {
        Self::new()
    }
}

impl DoppelgangerState {
    /// Create a new doppelgänger state starting in grace period
    pub fn new() -> Self {
        Self::GracePeriod
    }

    /// Check if in grace period
    #[cfg(test)]
    pub fn is_grace_period(&self) -> bool {
        matches!(self, Self::GracePeriod)
    }

    /// Check if actively monitoring
    pub fn is_monitoring(&self) -> bool {
        matches!(self, Self::Monitoring)
    }

    /// Check if monitoring is completed
    #[cfg(test)]
    pub fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }

    /// Transition from grace period to monitoring
    pub(crate) fn end_grace_period(&mut self) {
        *self = Self::Monitoring;
    }

    /// Transition from monitoring to completed
    pub fn end_monitoring(&mut self) {
        *self = Self::Completed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_lifecycle() {
        let mut state = DoppelgangerState::new();

        // Start in grace period
        assert_eq!(state, DoppelgangerState::GracePeriod);
        assert!(state.is_grace_period());
        assert!(!state.is_monitoring());
        assert!(!state.is_completed());

        // Transition to monitoring
        state.end_grace_period();
        assert_eq!(state, DoppelgangerState::Monitoring);
        assert!(!state.is_grace_period());
        assert!(state.is_monitoring());
        assert!(!state.is_completed());

        // Complete monitoring
        state.end_monitoring();
        assert_eq!(state, DoppelgangerState::Completed);
        assert!(!state.is_grace_period());
        assert!(!state.is_monitoring());
        assert!(state.is_completed());
    }
}
