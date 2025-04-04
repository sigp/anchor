#![no_main]

mod setup;
use libfuzzer_sys::fuzz_target;
use setup::RECEIVER;
use ssv_types::message::SignedSSVMessage;
use ssz::Encode;

// Fuzz message validation
fuzz_target!(|msg: SignedSSVMessage| {
    let encoded = msg.as_ssz_bytes();
    let _ = RECEIVER.validate(&encoded);
});
