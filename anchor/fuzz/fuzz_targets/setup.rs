#![allow(dead_code)]
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use database::NetworkDatabase;
use message_receiver::{NetworkMessageReceiver, Outcome};
use message_sender::{MessageSender, NetworkMessageSender};
use message_validator::{DutiesProvider, Validator};
use openssl::rsa::Rsa;
use qbft::{
    Config, ConfigBuilder, DefaultLeaderFunction, InstanceHeight, Qbft, UnsignedWrappedQbftMessage,
};
use qbft_manager::QbftManager;
use signature_collector::SignatureCollectorManager;
use slot_clock::{ManualSlotClock, SlotClock};
use ssv_types::{
    consensus::BeaconVote, domain_type::DomainType, msgid::MessageId, OperatorId, ValidatorIndex,
};
use subnet_tracker::SubnetId;
use task_executor::TaskExecutor;
use tempfile::tempdir;
use tokio::sync::mpsc;
use types::{Epoch, Hash256, Slot};

// We do not have any duties, so mock the duties provider
pub(crate) struct MockDutiesProvider {}
impl DutiesProvider for MockDutiesProvider {
    fn is_validator_in_sync_committee(
        &self,
        _committee_period: u64,
        _validator_index: ValidatorIndex,
    ) -> bool {
        true
    }

    fn is_epoch_known_for_proposers(&self, _epoch: Epoch) -> bool {
        true
    }

    fn is_validator_proposer_at_slot(&self, _slot: Slot, _validator_index: ValidatorIndex) -> bool {
        true
    }
}

type MessageQueue = Arc<Mutex<VecDeque<(OperatorId, UnsignedWrappedQbftMessage)>>>;
type QbftSendFn = Box<dyn FnMut(UnsignedWrappedQbftMessage) + Send + Sync>;

pub static VALIDATOR: LazyLock<Arc<Validator<ManualSlotClock, MockDutiesProvider>>> =
    LazyLock::new(setup_test_message_validator);

pub static RUNTIME: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().unwrap());

pub static RECEIVER: LazyLock<Arc<NetworkMessageReceiver<ManualSlotClock, MockDutiesProvider>>> =
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
        block_root: Hash256::default(),
        source: types::Checkpoint::default(),
        target: types::Checkpoint::default(),
    };

    Qbft::new(config, data, MessageId::from([0; 56]), send_message)
}

// Sets up a real Validator for fuzzing
pub fn setup_test_message_validator() -> Arc<Validator<ManualSlotClock, MockDutiesProvider>> {
    let slot_clock = ManualSlotClock::new(
        Slot::new(0),
        Duration::from_secs(0),
        Duration::from_secs(12),
    );
    let rsa = Rsa::private_key_from_pem(TESTING_KEY.as_bytes()).expect("Key is valid");
    let public_key =
        Rsa::from_public_components(rsa.n().to_owned().unwrap(), rsa.e().to_owned().unwrap())
            .unwrap();

    let tempdir = tempdir().unwrap();
    let file = tempdir.path().join("db.sqlite");
    let path = Path::new(&file);
    let db = NetworkDatabase::new(path, &public_key).expect("Database construction will not fail");

    let duties_provider = MockDutiesProvider {};

    Arc::new(Validator::new(
        db.watch(),
        32,
        256,
        duties_provider.into(),
        slot_clock.clone(),
    ))
}

// Sets up a real NetworkMessageReceiver for fuzzing
pub fn setup_test_message_receiver(
) -> Arc<NetworkMessageReceiver<ManualSlotClock, MockDutiesProvider>> {
    let handle = tokio::runtime::Handle::current();
    let (_signal, exit) = async_channel::bounded(1);
    let (shutdown_tx, _) = futures::channel::mpsc::channel(1);
    let executor = TaskExecutor::new(handle, exit, shutdown_tx, "test_executor".into());

    let processor_config = processor::Config {
        max_workers: 2,
        queue_size: Default::default(),
    };
    let processor_senders = processor::spawn(processor_config, executor);

    let slot_clock = ManualSlotClock::new(
        types::Slot::new(0),
        Duration::from_secs(0),
        Duration::from_secs(12),
    );
    let rsa = Rsa::private_key_from_pem(TESTING_KEY.as_bytes()).expect("Key is valid");
    let public_key =
        Rsa::from_public_components(rsa.n().to_owned().unwrap(), rsa.e().to_owned().unwrap())
            .unwrap();
    let tempdir = tempdir().unwrap();
    let file = tempdir.path().join("db.sqlite");
    let path = Path::new(&file);
    let db = NetworkDatabase::new(path, &public_key).expect("Database construction will not fail");

    let (network_tx, _) = mpsc::channel::<(SubnetId, Vec<u8>)>(9001);

    let operator_id = OperatorId(1);
    let domain_type = DomainType([0, 0, 0, 0]);

    let duties_provider = MockDutiesProvider {};

    let message_validator = Arc::new(Validator::new(
        db.watch(),
        32,
        256,
        duties_provider.into(),
        slot_clock.clone(),
    ));

    let network_message_sender: Arc<dyn MessageSender> = Arc::new(
        NetworkMessageSender::new(
            processor_senders.clone(),
            network_tx.clone(),
            rsa.clone(),
            operator_id,
            Some(message_validator.clone()),
            128,
        )
        .unwrap(),
    );

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

// Default key for determinisitic setup. This was generated only for testing and is not used
// anywhere
const TESTING_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCXpzq9yJPBj5b7
A2kqQ3CxDUxCmkcRpZz+eJq4314yxNVMyAjXEtTv62gXxSmru2se7eFky15Evw9a
/OAnsmlDEW64Dt6n6ZanHNXYFu2Y7enEpUn4OmVim2KIq/T2M3nYVtxsekPflb0Q
OTWXiqszkvNmxDJ95Jc6WvhfubWl3EBOZ3os2/xrS3zoA1+bIBLRtwzAM1O7uxwT
Pg7nts/hqvvFS0njf4CMl6MNoac5GrE7RSBStDMkJZbay5xOEUhUxZGuOMY8ppX5
vhHodvuNktSrWRxKG+D3Sh7CtyqOd0oDFR5w4EpU8flwBvD19vTj6bqbqpJqjR7Y
PvH7/MvNAgMBAAECggEACYCJQLJG98jRx/aQaf0scXDeMgoioStu/nl7ZaaxQJ3D
/k9GUTDf4LcvoCr9ZReVgFbyrsht/AFl/+h6Tw0cZVRmoJJl8cAZbW0exQRiwhie
HhvRL67RAsWuUyvwaatosLJ4ld9vTfIUP5D7bPxbpRv0XksKyDKWJdTksnLGEUH2
ni91JuxfAouJgoAWAssQrZPtsT99KbEJxD8q9KDa5ODT6wQmaTmD6gDSFzXcDNBa
Bkpc9XaJSaEFtjZIKza5YftRhVVK6LDYqeJMk5Atzbihf33dmZrhmT+zCP2HvIOk
c6gXLqPrRe7gTSLN+cbpimzGZL1+Nkks0xsk/6UnbQKBgQC/zw4Nj4HhcbCrljkz
BGrjNqSHsszEenrEn9N1r5zym2hDUTcLLjZAHvhypVpdqy+Z9xifDgD8nXSbiF6k
b/fv9aP18P9YU8k1n6MjkdVsr3z4bmMZD1alVVp9gfLJHMJ52+sptg4XpMp75q5v
XIcSDF9rTMmcdNGB7MbdYCqmCwKBgQDKZ+dbjPGqKSNdlpmJJ4Zw6+9CKxskDkNj
fGsq8pR1vWTlpA7WCDoymOzMyB+EZ81HxT2c5aNaF75X6Db/TAljzHUdxCfv/8fT
RDRWkDBMz62MGWx6lifYr8HvjdQ7lB3c2i2qPzsQWLLqaCw8Z3ya4J9kmOL6wNeT
wjiL9280hwKBgFWJVrEBcGBDPRAn+/YeYDRXZ+QD/oEYRattwvVWjV07pLFwhGV+
BD9wEEfAKZ5f+uhkYxx7OEFvTlMV627VZ/Igzy+ce6K+Kpq5SB1SqaTAVbDMOXEx
f+hXOfWCf+zj4G5LfoGpaHtux8WdR+jtkGaiEeNd6QLWrZ+NIdoTSrGlAoGBALXx
8Oc7K4HquP/IAPxpq1CWxdyVIzCmIa2siilxJkMwnSJQ94UuoCIblcH/o1VCeiWq
CFihlNXHwjMDa2zSzR4JDL5VNhFnvBkNln653rEtfrQRppILqIYAeDT/KWjlHHML
LUF81Xs8QJi2TA2AeWI/yQiE5oTCFQed73biVfTBAoGACEDOZi7v9Ncj03XY4UNl
IxIjMgIIlkjifLjr2MVi/qEx67109rsdZAGGAb1YCukel/0NAXmxjXROxj1lHHPe
xfn7l4RWwIGqi3yZtxfpKB29mjBaY0BRL6XPhGe2MAfydeXMdwk6QrZPcYroOJ+t
SPdvWXU4osCd7vgiJvAP4ek=
-----END PRIVATE KEY-----";
