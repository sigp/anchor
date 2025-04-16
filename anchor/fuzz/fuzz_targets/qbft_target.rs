#![no_main]

mod setup;
use libfuzzer_sys::fuzz_target;
use qbft::WrappedQbftMessage;
use setup::QBFT;

// Fuzz message validation
fuzz_target!(|msg: WrappedQbftMessage| {
    let mut qbft_locked = QBFT.lock().unwrap();
    qbft_locked.receive(msg)
});
