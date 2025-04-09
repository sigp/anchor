#![allow(dead_code)]
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use database::NetworkDatabase;
use message_receiver::{NetworkMessageReceiver, Outcome};
use message_sender::NetworkMessageSender;
use message_validator::Validator;
use openssl::rsa::Rsa;
use qbft::{
    Config, ConfigBuilder, DefaultLeaderFunction, InstanceHeight, Qbft, UnsignedWrappedQbftMessage,
};
use qbft_manager::QbftManager;
use signature_collector::SignatureCollectorManager;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{consensus::BeaconVote, domain_type::DomainType, msgid::MessageId, OperatorId};
use subnet_tracker::SubnetId;
use task_executor::TaskExecutor;
use tokio::sync::mpsc;
use types::{Hash256, Slot};

type MessageQueue = Arc<Mutex<VecDeque<(OperatorId, UnsignedWrappedQbftMessage)>>>;
type QbftSendFn = Box<dyn FnMut(UnsignedWrappedQbftMessage) + Send + Sync>;

pub static VALIDATOR: LazyLock<Arc<Validator<ManualSlotClock>>> =
    LazyLock::new(setup_test_message_validator);

pub static RUNTIME: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

pub static RECEIVER: LazyLock<Arc<NetworkMessageReceiver<ManualSlotClock>>> =
    LazyLock::new(setup_test_message_receiver);

pub static QBFT: LazyLock<Arc<Mutex<Qbft<DefaultLeaderFunction, BeaconVote, QbftSendFn>>>> =
    LazyLock::new(|| {
        let msg_queue: MessageQueue = Arc::new(Mutex::new(VecDeque::new()));
        let id = OperatorId::from(1);

        let send_message: QbftSendFn = Box::new(move |message| {
            let mut queue = msg_queue.lock().unwrap();
            queue.push_back((id, message));
        });

        Arc::new(Mutex::new(setup_qbft_instance(send_message)))
    });

// Setup a new Qbft instance
pub fn setup_qbft_instance(
    send_message: QbftSendFn,
) -> Qbft<DefaultLeaderFunction, BeaconVote, QbftSendFn> {
    let config: Config<DefaultLeaderFunction> = ConfigBuilder::new(
        1.into(),
        InstanceHeight::default(),
        (1..=4).map(OperatorId::from).collect(),
    )
    .build()
    .unwrap();

    let data = BeaconVote {
        block_root: Hash256::random(),
        source: types::Checkpoint::default(),
        target: types::Checkpoint::default(),
    };

    Qbft::new(config, data, MessageId::from([0; 56]), send_message)
}

// Sets up a real Validator for fuzzing
pub fn setup_test_message_validator() -> Arc<Validator<ManualSlotClock>> {
    let slot_clock = ManualSlotClock::new(
        Slot::new(0),
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

// Sets up a real NetworkMessageReceiver for fuzzing
pub fn setup_test_message_receiver() -> Arc<NetworkMessageReceiver<ManualSlotClock>> {
    let handle = tokio::runtime::Handle::current();
    let (_signal, exit) = async_channel::bounded(1);
    let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown_tx, "test_executor".into());

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
