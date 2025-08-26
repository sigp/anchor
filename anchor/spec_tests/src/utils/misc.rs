use sha2::{Digest, Sha256};
use types::Hash256;

/// Hash data using SHA256 and return as Hash256
pub fn hash_data(data: &[u8]) -> Hash256 {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let hash_bytes: [u8; 32] = hasher.finalize().into();
    Hash256::from(hash_bytes)
}

/// Calculate QBFT quorum size for a committee
/// Formula: quorum = n - f where f = (n-1)/3
pub fn calculate_quorum(committee_size: usize) -> usize {
    let f = (committee_size - 1) / 3;
    committee_size - f
}
