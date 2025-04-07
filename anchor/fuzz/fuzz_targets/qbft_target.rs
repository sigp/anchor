#![no_main]

mod setup;
use libfuzzer_sys::fuzz_target;
use qbft::WrappedQbftMessage;
use setup::QBFT;

// Fuzz message validation
fuzz_target!(|msg: WrappedQbftMessage|  QBFT.lock().unwrap().receive(msg) );
