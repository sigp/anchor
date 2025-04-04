use std::{path::Path, sync::Arc, time::Duration};

use database::NetworkDatabase;
use message_receiver::{NetworkMessageReceiver, Outcome};
use message_sender::NetworkMessageSender;
use message_validator::Validator;
use openssl::rsa::Rsa;
use qbft_manager::QbftManager;
use signature_collector::SignatureCollectorManager;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{domain_type::DomainType, OperatorId};
use std::sync::LazyLock;
use subnet_tracker::SubnetId;
use tokio::sync::mpsc;

// Create static runtime and receiver that will be initialized only once
pub static RUNTIME: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

pub static RECEIVER: LazyLock<Arc<NetworkMessageReceiver<ManualSlotClock>>> =
    LazyLock::new(setup_test_message_receiver);

// Sets up a real NetworkMessageReceiver for fuzzing
pub fn setup_test_message_receiver() -> Arc<NetworkMessageReceiver<ManualSlotClock>> {
    let handle = tokio::runtime::Handle::current();
    let (_signal, exit) = async_channel::bounded(1);
    let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
    let executor =
        task_executor::TaskExecutor::new(handle, exit, shutdown_tx, "test_executor".into());

    let processor_config = processor::Config { max_workers: 2 };
    let processor_senders = processor::spawn(processor_config, executor);

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

    let (network_tx, _) = mpsc::channel::<(SubnetId, Vec<u8>)>(9001);

    let operator_id = OperatorId(1);
    let domain_type = DomainType([0, 0, 0, 0]);

    let message_validator = Arc::new(Validator::new(db.watch(), 32, slot_clock.clone()));
    let network_message_sender = NetworkMessageSender::new(
        processor_senders.clone(),
        network_tx.clone(),
        rsa.clone(),
        operator_id,
        Some(message_validator.clone()),
        128,
    )
    .unwrap();

    let (outcome_tx, _) = mpsc::channel::<Outcome>(9000);

    let signature_collector = SignatureCollectorManager::new(
        processor_senders.clone(),
        operator_id,
        domain_type.clone(),
        network_message_sender.clone(),
        slot_clock.clone(),
    )
    .unwrap();

    let qbft_manager = QbftManager::new(
        processor_senders.clone(),
        operator_id,
        slot_clock,
        network_message_sender,
        domain_type,
    )
    .unwrap();

    NetworkMessageReceiver::new(
        processor_senders,
        qbft_manager,
        signature_collector,
        db.watch(),
        outcome_tx,
        message_validator,
    )
}
