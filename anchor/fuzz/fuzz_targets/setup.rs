use std::{
    path::Path,
    sync::{Arc, LazyLock},
    time::Duration,
};

use database::NetworkDatabase;
use message_validator::Validator;
use openssl::rsa::Rsa;
use slot_clock::{ManualSlotClock, SlotClock};

pub static RECEIVER: LazyLock<Arc<Validator<ManualSlotClock>>> =
    LazyLock::new(setup_test_message_receiver);

// Sets up a real NetworkMessageReceiver for fuzzing
pub fn setup_test_message_receiver() -> Arc<Validator<ManualSlotClock>> {
    let slot_clock = ManualSlotClock::new(
        types::Slot::new(0),
        Duration::from_secs(0),
        Duration::from_secs(12),
    );
    let rsa = Rsa::generate(2048).expect("Keygen will not fail");
    let public_key =
        Rsa::from_public_components(rsa.n().to_owned().unwrap(), rsa.e().to_owned().unwrap())
            .unwrap();
    let path = Path::new("keysplit.sqlite");
    let db = NetworkDatabase::new(path, &public_key).expect("Database construction will not fail");

    Arc::new(Validator::new(db.watch(), 32, slot_clock.clone()))
}
