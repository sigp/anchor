// All checks to run on the simulation to ensure that is
// operating in an expected manner
use crate::local_network::SsvLocalNetwork;
use node_test_rig::eth2::types::{BlockId, StateId};
use std::time::Duration;
use types::{Epoch, EthSpec, ExecPayload, ExecutionBlockHash, Slot};

// Checks that the chain has made the first possible finalization.
//
// Intended to be run as soon as chain starts.
pub async fn verify_first_finalization<E: EthSpec>(
    network: SsvLocalNetwork<E>,
    slot_duration: Duration,
) -> Result<(), String> {
    epoch_delay(Epoch::new(4), slot_duration, E::slots_per_epoch()).await;
    verify_all_finalized_at(network, Epoch::new(2)).await?;
    Ok(())
}

// Delays for `epochs`, plus half a slot extra.
pub async fn epoch_delay(epochs: Epoch, slot_duration: Duration, slots_per_epoch: u64) {
    let duration = slot_duration * (epochs.as_u64() * slots_per_epoch) as u32 + slot_duration / 2;
    tokio::time::sleep(duration).await
}

// Delays for `slots`, plus half a slot extra.
async fn slot_delay(slots: Slot, slot_duration: Duration) {
    let duration = slot_duration * slots.as_u64() as u32 + slot_duration / 2;
    tokio::time::sleep(duration).await;
}

// Verifies that all beacon nodes in the given network have a head state that has a finalized
// epoch of `epoch`.
pub async fn verify_all_finalized_at<E: EthSpec>(
    network: SsvLocalNetwork<E>,
    epoch: Epoch,
) -> Result<(), String> {
    let epochs = {
        let mut epochs = Vec::new();
        for remote_node in network.remote_nodes()? {
            epochs.push(
                remote_node
                    .get_beacon_states_finality_checkpoints(StateId::Head)
                    .await
                    .map(|body| body.unwrap().data.finalized.epoch)
                    .map_err(|e| format!("Get head via http failed: {:?}", e))?,
            );
        }
        epochs
    };

    if epochs.iter().any(|node_epoch| *node_epoch != epoch) {
        Err(format!(
            "Nodes are not finalized at epoch {}. Finalized epochs: {:?}",
            epoch, epochs
        ))
    } else {
        Ok(())
    }
}

// Verifies that there's been a block produced at every slot up to and including `slot`.
pub async fn verify_full_block_production_up_to<E: EthSpec>(
    network: SsvLocalNetwork<E>,
    slot: Slot,
    slot_duration: Duration,
) -> Result<(), String> {
    slot_delay(slot, slot_duration).await;
    let beacon_nodes = network.beacon_nodes.read();
    let beacon_chain = beacon_nodes[0].client.beacon_chain().unwrap();
    let num_blocks = beacon_chain
        .chain_dump()
        .unwrap()
        .iter()
        .take_while(|s| s.beacon_block.slot() <= slot)
        .count();
    if num_blocks != slot.as_usize() + 1 {
        return Err(format!(
            "There wasn't a block produced at every slot, got: {}, expected: {}",
            num_blocks,
            slot.as_usize() + 1
        ));
    }
    Ok(())
}

// Verify that all sync aggregates from `sync_committee_start_slot` until `upto_slot`
// have full aggregates.
pub async fn verify_full_sync_aggregates_up_to<E: EthSpec>(
    network: SsvLocalNetwork<E>,
    sync_committee_start_slot: Slot,
    upto_slot: Slot,
    slot_duration: Duration,
) -> Result<(), String> {
    slot_delay(upto_slot, slot_duration).await;
    let remote_nodes = network.remote_nodes()?;
    let remote_node = remote_nodes.first().unwrap();

    for slot in sync_committee_start_slot.as_u64()..=upto_slot.as_u64() {
        let sync_aggregate_count = remote_node
            .get_beacon_blocks::<E>(BlockId::Slot(Slot::new(slot)))
            .await
            .map(|resp| {
                resp.unwrap()
                    .data
                    .message()
                    .body()
                    .sync_aggregate()
                    .map(|agg| agg.num_set_bits())
            })
            .map_err(|e| format!("Error while getting beacon block: {:?}", e))?
            .map_err(|_| format!("Altair block {} should have sync aggregate", slot))?;

        if sync_aggregate_count != E::sync_committee_size() {
            return Err(format!(
                "Sync aggregate at slot {} was not full, got: {}, expected: {}",
                slot,
                sync_aggregate_count,
                E::sync_committee_size()
            ));
        }
    }

    Ok(())
}

// Verify that the first merged PoS block got finalized.
pub async fn verify_transition_block_finalized<E: EthSpec>(
    network: SsvLocalNetwork<E>,
    transition_epoch: Epoch,
    slot_duration: Duration,
    should_verify: bool,
) -> Result<(), String> {
    if !should_verify {
        return Ok(());
    }
    epoch_delay(transition_epoch + 2, slot_duration, E::slots_per_epoch()).await;
    let mut block_hashes = Vec::new();
    for remote_node in network.remote_nodes()?.iter() {
        let execution_block_hash: ExecutionBlockHash = remote_node
            .get_beacon_blocks::<E>(BlockId::Finalized)
            .await
            .map(|body| body.unwrap().data)
            .map_err(|e| format!("Get state root via http failed: {:?}", e))?
            .message()
            .execution_payload()
            .map(|payload| payload.block_hash())
            .map_err(|e| format!("Execution payload does not exist: {:?}", e))?;
        block_hashes.push(execution_block_hash);
    }

    let first = block_hashes[0];
    if block_hashes.iter().all(|&item| item == first) {
        Ok(())
    } else {
        Err(format!(
            "Terminal block not finalized on all nodes Finalized block hashes:{:?}",
            block_hashes
        ))
    }
}

// Ensure all validators have attested correctly.
pub async fn check_attestation_correctness<E: EthSpec>(
    network: SsvLocalNetwork<E>,
    start_epoch: u64,
    upto_epoch: u64,
    slot_duration: Duration,
    // Select which node to query. Will use this node to determine the global network performance.
    node_index: usize,
    acceptable_attestation_performance: f64,
) -> Result<(), String> {
    epoch_delay(Epoch::new(upto_epoch), slot_duration, E::slots_per_epoch()).await;

    let remote_node = &network.remote_nodes()?[node_index];

    let results = remote_node
        .get_lighthouse_analysis_attestation_performance(
            Epoch::new(start_epoch),
            Epoch::new(upto_epoch - 2),
            "global".to_string(),
        )
        .await
        .map_err(|e| format!("Unable to get attestation performance: {e}"))?;

    let mut active_successes: f64 = 0.0;
    let mut head_successes: f64 = 0.0;
    let mut target_successes: f64 = 0.0;
    let mut source_successes: f64 = 0.0;

    let mut total: f64 = 0.0;

    for result in results {
        for epochs in result.epochs.values() {
            total += 1.0;

            if epochs.active {
                active_successes += 1.0;
            }
            if epochs.head {
                head_successes += 1.0;
            }
            if epochs.target {
                target_successes += 1.0;
            }
            if epochs.source {
                source_successes += 1.0;
            }
        }
    }
    let active_percent = active_successes / total * 100.0;
    let head_percent = head_successes / total * 100.0;
    let target_percent = target_successes / total * 100.0;
    let source_percent = source_successes / total * 100.0;

    eprintln!("Total Attestations: {}", total);
    eprintln!("Active: {}: {}%", active_successes, active_percent);
    eprintln!("Head: {}: {}%", head_successes, head_percent);
    eprintln!("Target: {}: {}%", target_successes, target_percent);
    eprintln!("Source: {}: {}%", source_successes, source_percent);

    if active_percent < acceptable_attestation_performance {
        return Err("Active percent was below required level".to_string());
    }
    if head_percent < acceptable_attestation_performance {
        return Err("Head percent was below required level".to_string());
    }
    if target_percent < acceptable_attestation_performance {
        return Err("Target percent was below required level".to_string());
    }
    if source_percent < acceptable_attestation_performance {
        return Err("Source percent was below required level".to_string());
    }

    Ok(())
}
