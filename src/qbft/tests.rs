#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialization() {
        let mut qbft = QBFT::new(0, 0);
        qbft.initialize();

        assert_eq!(qbft.out_events.len(), 1)
    }
}
