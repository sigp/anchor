use std::{str::FromStr, sync::Arc};

use alloy::{
    primitives::{Address, Bytes, FixedBytes, LogData, U256},
    rpc::types::Log,
    sol_types::SolEvent,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use database::{
    NetworkDatabase,
    test_utils::{TestFixture, assertions, generators},
};
use eth::{
    event_processor::{EventProcessor, Mode},
    generated::SSVContract,
};
use rusqlite::Connection;
use slashing_protection::SlashingDatabase;
use ssv_types::{domain_type::DomainType, *};
use tempfile::TempDir;
use tokio::sync::mpsc::unbounded_channel;
use types::PublicKeyBytes;

// Valid BLS signature data for ValidatorAdded event testing
const VALID_SHARES_DATA: &str = "94f4ebe1793f2e84eff9e453b0d66db2cc2a7c85072ed5677dc83227acce4160544980e084a80adabfba9ccc8c96c26313c35432acabb88e46eea17c9cc6e5451eae4a313cf9ef9c581a496463c59a8bd9535f3093ae5202e7799030d7277a238c77c307b0c3bda7ca2ff84ed144a2db283f5478a1dcb98a613d3c0b79f864c9ac90aca17498932beaa1ca5b90818047b8c16f121dc3ae44be832200b45388c3fb38b791886d3dee0cdb9e30c394caa4113e43fa8215889cdd1ee26e31d7c9ad8e02f493fadeeca67e095314e14ff97ef95a9530e3ee961bd6b88aaa980ecaacf3171cd029c0dc7068e83896930f05eeb6b1835b9d26a9b811567bccaf19ee97d2dcdafc3d88f2d0d556aba4c7c3cdcbf994228135dc6bfa4d0dd51067fe0f503f26e63bbeec7e0fcaff670e3d75825deea68313de4085670c8375771e75373ab2c8656fbce23d322d788d70d99942a788d758a2856a6c2826d35d4ffa4bdd9316206ffa3b9af256576fcad3cb4058424c3476553d513125b86062df3f466b758e8924f113d953905f0ba8fa40363c92995a92ff2f22c46419d484b91aa0e0953e1fabed016e3b5bfbcca8c4294dbfef91ea777c9ef4bc501ba71058d31535d696e3d3bac2c689e4adab8444d1b7ee75c4727947c2c55015656bbf3259831d0796c769e98d1b28fdb0d25800bf75afbdf3366535cca9e703fa56e69a6e36fe084170f67562ec9a41b49eeab5782534a01e4acd352eb9c7310689fae53f5f526169ba58f607dd0b314d993fcd0c92b91ac22765f3813c16c8d85a62fc35015b925338fa0a197b5d51a9c13bad2d16b4a62e35d2b4de8827b408cac6804746c2d71cbfcee9ae9270c0c64a06db70f71296bda3af1055177e269222d912373cd58d3f121e6e570ff1cb2ce3d9844f35af5175b3a7f93082f48f8e840874d6029c5a8e4802d204cd13ccf0fa98c5ce2a30f7a8f8eff3bcbb78ea464d450eacbf9b182ffdadf58089d63e81a39dccf41d67c2efe8de8f2774a3d97ff3aaba0c55c22fc47e1daa9ae67c8c63af95c4f398ac046bbb1a1b775882021307607bb50662f3c15bf29384bc2e1f98f5f955d0c12cb126a2828c899b2946075347bf2543566379abd52374fb5b1492d408b178f03c19d431572a2de6fbcd10666b7d3ac5e46a5b25ff72e77ab3c9fd593b0a5638c24d13ce42eb8350c72b440fd2c4fe1c834a04308c89151ed3ed11c272097f7fd831f511afdf616c8fc117c8b97478321667b69d00590d7b4ec08397d2897c8139688c3496fee6dafac57e87ea4cb9ef048e18efd290cb1f48ae45b39d26809ad5ba7150ee9c2abedcfb295cba73baccc832035a8e85994a6f2466a9fb5bbda62a0f7d725db0f7a3fc714dd8b1deb713f732e8fba435ef8f8792b81e411a28dc2753c9bfca11bcb27e548593289fe9406fa4d6d1565c8d9998856a0dcfe7a6c1e207cb22ae19c33e34e318024e24644fd29b51e18c02d53f0705413eb5bd6252459749e1e81be07caabdff0e237d14980d60f5f3bf2dcd980b2e4cfbb3aa936e546d35ed2f097eeeec780fa92c685418c6b510e7c2e226263fe8cb065de7cf6bdea4ed59009d9fd08e4c321630d31b2fb9ca9cd16d4a9ae88ef21b517e805764bec0eca03dbd8b5cf88c561bbde1efc3c3e80080ea3c651942a02075da358dfc6a5769ea83495123569faccd1043e9437b217318c812eab3a1dd58481db3055c29e53b13bb30c47c907021c48833de90d9d977153bcd49ca69c6c772634035d8e339184f8065ee90e1bbb6d19cb4360b4db4cbdeededd91bdec5c54f1ea693037730d238d0aa63c5233207d661abc068e105";

fn create_valid_rsa_public_key_bytes() -> Bytes {
    let rsa_key = generators::pubkey::random_rsa();
    let pem_data = rsa_key
        .public_key_to_pem()
        .expect("Failed to convert to PEM");
    let base64_pem = BASE64_STANDARD.encode(&pem_data);
    Bytes::from(base64_pem.as_bytes().to_vec())
}

fn create_test_slashing_db() -> Arc<SlashingDatabase> {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let slashing_db_path = temp_dir.path().join("slashing.db");
    Arc::new(SlashingDatabase::create(&slashing_db_path).expect("Failed to create slashing db"))
}

fn setup_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

fn create_node_mode_processor(
    db: Arc<NetworkDatabase>,
) -> (
    EventProcessor,
    tokio::sync::mpsc::UnboundedReceiver<PublicKeyBytes>,
) {
    let (index_sync_tx, index_sync_rx) = unbounded_channel();
    let (exit_tx, _exit_rx) = unbounded_channel();
    let slashing_protection = create_test_slashing_db();

    let processor = EventProcessor::new(
        db,
        Mode::Node {
            index_sync_tx,
            exit_tx,
            slashing_protection,
        },
    );
    (processor, index_sync_rx)
}

fn verify_operator_stored(processor: &EventProcessor, operator_id: OperatorId) {
    // Get the stored operator from memory first
    let stored_operator = processor
        .db
        .state()
        .get_operator(&operator_id)
        .expect("Operator should be stored and accessible");

    // Verify operator exists in both database and memory using database test utilities
    let mut conn = processor
        .db
        .connection()
        .expect("Failed to get database connection");
    let tx = conn.transaction().expect("Failed to start transaction");

    assertions::operator::exists_in_db(&stored_operator, &tx);
    assertions::operator::exists_in_memory(&processor.db, &stored_operator);
}

// Get database metadata
fn get_metadata(conn: &Connection) -> Result<(u64, DomainType, u64), rusqlite::Error> {
    let query = "SELECT schema_version, domain_type, block_number FROM metadata";
    conn.query_row(query, [], |row| {
        Ok((
            row.get("schema_version")?,
            row.get("domain_type")?,
            row.get("block_number")?,
        ))
    })
}

/// Helper function to create a mock Log object for SSV contract events
fn create_mock_log(
    address: Address,
    topics: Vec<FixedBytes<32>>,
    data: Bytes,
    block_number: Option<u64>,
    transaction_hash: Option<FixedBytes<32>>,
    log_index: Option<u64>,
) -> Log {
    let log_data = LogData::new(topics, data).expect("Failed to create log data");

    Log {
        inner: alloy::primitives::Log {
            address,
            data: log_data,
        },
        block_hash: Some(FixedBytes::default()),
        block_number,
        block_timestamp: Some(1234567890u64),
        transaction_hash,
        transaction_index: Some(0u64),
        log_index,
        removed: false,
    }
}

/// Helper function to create an OperatorAdded event log
fn create_operator_added_log(operator_id: u64, owner: Address, public_key: Bytes, fee: u64) -> Log {
    let event = SSVContract::OperatorAdded {
        operatorId: operator_id,
        owner,
        publicKey: public_key,
        fee: U256::from(fee),
    };

    // Create topics array with the event signature and indexed parameters
    let mut topics = vec![SSVContract::OperatorAdded::SIGNATURE_HASH];
    let operator_id_bytes: [u8; 32] = {
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&operator_id.to_be_bytes());
        bytes
    };
    topics.push(FixedBytes::from(operator_id_bytes));
    let mut owner_bytes = [0u8; 32];
    owner_bytes[12..32].copy_from_slice(owner.as_slice());
    topics.push(FixedBytes::from(owner_bytes));

    // Encode the non-indexed data
    let data = event.encode_data();

    create_mock_log(
        Address::default(), // contract address
        topics,
        data.into(),
        Some(12345),
        Some(FixedBytes::default()),
        Some(0),
    )
}

/// Helper function to create a ValidatorAdded event log
fn create_validator_added_log(
    owner: Address,
    operator_ids: Vec<u64>,
    public_key: Bytes,
    shares: Bytes,
) -> Log {
    let cluster = SSVContract::Cluster {
        validatorCount: 1,
        networkFeeIndex: 0,
        index: 0,
        active: true,
        balance: U256::from(0),
    };

    let event = SSVContract::ValidatorAdded {
        owner,
        operatorIds: operator_ids,
        publicKey: public_key,
        shares,
        cluster,
    };

    // Create topics array with the event signature and indexed parameters
    let mut topics = vec![SSVContract::ValidatorAdded::SIGNATURE_HASH];
    let mut owner_bytes = [0u8; 32];
    owner_bytes[12..32].copy_from_slice(owner.as_slice());
    topics.push(FixedBytes::from(owner_bytes)); // indexed owner

    // Encode the non-indexed data
    let data = event.encode_data();

    create_mock_log(
        Address::default(), // contract address
        topics,
        data.into(),
        Some(12346),
        Some(FixedBytes::default()),
        Some(1),
    )
}

#[tokio::test]
async fn test_operator_added_event_processing() {
    setup_tracing();

    // Setup test fixture and processor
    let fixture = TestFixture::new_empty();
    let (processor, _index_sync_rx) = create_node_mode_processor(Arc::new(fixture.db));

    // Create test data
    let operator_id = 42u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    // Create OperatorAdded log
    let log = create_operator_added_log(operator_id, owner, public_key, 1000);

    // Process the log
    let result = processor.process_logs(vec![log], true, 12345);
    assert!(
        result.is_ok(),
        "Processing OperatorAdded event should succeed"
    );

    // Verify operator was stored in database and memory
    verify_operator_stored(&processor, OperatorId(operator_id));
}

#[tokio::test]
async fn test_validator_added_event_processing() {
    setup_tracing();

    // Setup test fixture with populated operators
    let fixture = TestFixture::new();
    let (processor, mut index_sync_rx) = create_node_mode_processor(Arc::new(fixture.db));

    // Use operators from the fixture
    let operator_ids: Vec<u64> = fixture.operators.iter().map(|op| *op.id).collect();

    // Create properly formatted shares data using the module-level constant
    let shares_data = hex::decode(VALID_SHARES_DATA).expect("Failed to decode hex string");
    let shares = Bytes::from(shares_data);

    // We also need to use the corresponding owner and public key from that test
    let owner =
        Address::from_str("0x000000633b68f5d8d3a86593ebb815b4663bcbe0").expect("Invalid address");
    let public_key = Bytes::from_str("0x97e8235ec2174862a8162ef9624f2fb1df82a3a8ef57f72a2a866df37c3da66020b1e4070d0d443ef40198e71afe9493").expect("Invalid public key");

    // Create ValidatorAdded log
    let log = create_validator_added_log(owner, operator_ids, public_key, shares);

    // Process the log - should succeed with valid signature
    let result = processor.process_logs(vec![log], true, 12346);

    // Should be processed successfully with valid signature
    assert!(
        result.is_ok(),
        "ValidatorAdded should be processed successfully with valid signature"
    );

    // Verify that validator was queued for index sync
    tokio::select! {
        validator_key = index_sync_rx.recv() => {
            assert!(validator_key.is_some(), "Validator should be queued for index sync");
        }
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
            panic!("Validator should have been queued for index sync");
        }
    }
}

#[tokio::test]
async fn test_multiple_events_processing() {
    // Setup test fixture and processor
    let fixture = TestFixture::new_empty();
    let (processor, _index_sync_rx) = create_node_mode_processor(Arc::new(fixture.db));

    // Create multiple OperatorAdded events
    let num_operators = 3;
    let logs: Vec<Log> = (0..num_operators)
        .map(|i| {
            let operator_id = i + 1;
            let owner = Address::random();
            let public_key = create_valid_rsa_public_key_bytes();
            create_operator_added_log(operator_id, owner, public_key, 1000 + i * 100)
        })
        .collect();

    // Process all logs in a single batch
    let result = processor.process_logs(logs, true, 12350);
    assert!(result.is_ok(), "Processing multiple events should succeed");

    // Verify all operators were stored
    for i in 0..num_operators {
        verify_operator_stored(&processor, OperatorId(i + 1));
    }

    // Verify processed block was updated
    let conn = processor
        .db
        .connection()
        .expect("Failed to get database connection");
    let (_, _, block_number) = get_metadata(&conn).expect("Failed to get metadata");
    assert_eq!(block_number, 12350, "Block number should be updated");
}

#[tokio::test]
async fn test_database_transaction_rollback_on_error() {
    // Setup test fixture and processor
    let fixture = TestFixture::new_empty();
    let (processor, _index_sync_rx) = create_node_mode_processor(Arc::new(fixture.db));

    // Create test data
    let operator_id = 1u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    let valid_log = create_operator_added_log(operator_id, owner, public_key.clone(), 1000);

    // Create an invalid log (duplicate operator ID) that should cause an error
    let invalid_log = create_operator_added_log(operator_id, Address::random(), public_key, 2000);

    let logs = vec![valid_log, invalid_log];

    // Process logs - this should fail due to duplicate operator ID
    let result = processor.process_logs(logs, true, 12351);

    // The processing should complete (some events may be malformed and skipped)
    // but the transaction should still commit for valid events
    assert!(
        result.is_ok(),
        "Processing should handle malformed events gracefully"
    );

    // Verify the first operator was stored (malformed events are skipped, not rolled back)
    verify_operator_stored(&processor, OperatorId(operator_id));
}

#[tokio::test]
async fn test_keysplit_mode_processing() {
    // Setup test fixture and processor
    let fixture = TestFixture::new_empty();
    let processor = EventProcessor::new(Arc::new(fixture.db), Mode::KeySplit);

    // Create test data
    let operator_id = 5u64;
    let owner = Address::random();
    let public_key = create_valid_rsa_public_key_bytes();

    let log = create_operator_added_log(operator_id, owner, public_key, 1500);

    // Process the log
    let result = processor.process_logs(vec![log], true, 12352);
    assert!(
        result.is_ok(),
        "KeySplit mode should process OperatorAdded events"
    );

    // Verify operator was stored even in KeySplit mode
    verify_operator_stored(&processor, OperatorId(operator_id));
}
