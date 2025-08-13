use std::collections::{HashMap, HashSet, hash_map::Entry};

use discv5::libp2p_identity::PeerId;
use libp2p::{
    Multiaddr,
    swarm::{
        FromSwarm, NewExternalAddrOfPeer,
        dial_opts::{DialOpts, PeerCondition},
    },
};
use network_utils::enr_ext::EnrExt;
use peer_store::{
    Store,
    memory_store::{MemoryStore, PeerRecord},
};
use rand::seq::SliceRandom;
use subnet_service::SubnetId;
use tracing::debug;

use super::{connection::ConnectionManager, types::ConnectActions};
use crate::{Enr, discovery};

/// Number of times we overdial when we need peers for a subnet
const PEER_OVERDIAL_FACTOR: usize = 2;

/// Minimum number of peers required per subnet
const MIN_PEERS_PER_SUBNET: usize = 6;

/// Manages peer discovery and subnet-based peer selection
pub struct PeerDiscovery;

impl PeerDiscovery {
    /// Process a discovered peer and return dial options if we should connect
    pub fn process_discovered_peer(
        enr: Enr,
        peer_store: &mut MemoryStore<Enr>,
        connection_manager: &ConnectionManager,
        needed_subnets: &HashSet<SubnetId>,
        blocked_peers: &HashSet<PeerId>,
    ) -> Option<DialOpts> {
        let id = enr.peer_id();

        let multiaddrs = enr.multiaddr();

        // Update peer store with the discovered peer
        for multiaddr in multiaddrs.iter() {
            peer_store.on_swarm_event(&FromSwarm::NewExternalAddrOfPeer(NewExternalAddrOfPeer {
                peer_id: id,
                addr: multiaddr,
            }));
        }
        peer_store.insert_custom_data(&id, enr.clone());

        // Check if we should dial this peer
        let should_dial =
            connection_manager.should_dial_peer(&id, peer_store, needed_subnets, blocked_peers);

        if should_dial {
            Some(Self::peer_to_dial_opts(&id, peer_store))
        } else {
            None
        }
    }

    /// Track a subnet as needed and return actions to find peers for it
    pub fn track_subnet_peers(
        subnet_id: SubnetId,
        needed_subnets: &mut HashSet<SubnetId>,
        peer_store: &MemoryStore<Enr>,
        connection_manager: &ConnectionManager,
        blocked_peers: &HashSet<PeerId>,
    ) -> ConnectActions {
        needed_subnets.insert(subnet_id);

        Self::determine_actions_for_subnets(
            &[subnet_id],
            peer_store,
            connection_manager,
            blocked_peers,
        )
    }

    /// Determine what actions to take for the given subnets
    pub fn determine_actions_for_subnets(
        subnets: &[SubnetId],
        peer_store: &MemoryStore<Enr>,
        connection_manager: &ConnectionManager,
        blocked_peers: &HashSet<PeerId>,
    ) -> ConnectActions {
        let mut actions = ConnectActions::none();
        let peer_counts = connection_manager.count_peers_for_subnets(subnets, peer_store);
        let mut subnet_needs = subnets
            .iter()
            .zip(peer_counts)
            .filter_map(|(subnet, count)| {
                let need = MIN_PEERS_PER_SUBNET.saturating_sub(count) * PEER_OVERDIAL_FACTOR;
                (need != 0).then_some((*subnet, need))
            })
            .collect::<HashMap<_, _>>();

        for (peer, record) in Self::candidate_peers(peer_store, &connection_manager.connected) {
            // Skip blocked peers
            if blocked_peers.contains(peer) {
                continue;
            }

            let Some(enr) = record.get_custom_data() else {
                continue;
            };

            let subnets = discovery::committee_bitfield(enr).unwrap_or_default();

            let mut relevant = false;
            for subnet in subnets
                .iter()
                .enumerate()
                .filter_map(|(subnet, subscribed)| {
                    subscribed.then_some(SubnetId::new(subnet as u64))
                })
            {
                let Entry::Occupied(mut need) = subnet_needs.entry(subnet) else {
                    continue;
                };
                relevant = true;
                *need.get_mut() -= 1;
                if need.get() == &0 {
                    need.remove();
                }
            }

            if relevant {
                actions.dial.push(Self::peer_to_dial_opts(peer, peer_store));
            }
        }

        actions.discover.extend(subnet_needs.into_keys());
        actions
    }

    /// Check if any subnets need more peers and return dial/discovery actions
    pub fn check_subnet_peers(
        needed_subnets: &HashSet<SubnetId>,
        peer_store: &MemoryStore<Enr>,
        connection_manager: &ConnectionManager,
        blocked_peers: &HashSet<PeerId>,
    ) -> Option<ConnectActions> {
        let actions = Self::determine_actions_for_subnets(
            &needed_subnets.iter().copied().collect::<Vec<_>>(),
            peer_store,
            connection_manager,
            blocked_peers,
        );

        if !actions.is_empty() {
            Some(actions)
        } else {
            None
        }
    }

    /// Get candidate peers that we could potentially dial
    fn candidate_peers<'a>(
        peer_store: &'a MemoryStore<Enr>,
        connected: &HashSet<PeerId>,
    ) -> Vec<(&'a PeerId, &'a PeerRecord<Enr>)> {
        let mut peers = peer_store
            .record_iter()
            .filter(|(peer, record)| {
                !connected.contains(peer) && record.addresses().next().is_some()
            })
            .collect::<Vec<_>>();
        peers.shuffle(&mut rand::rng());
        peers
    }

    /// Convert a peer ID to dial options
    fn peer_to_dial_opts(peer: &PeerId, peer_store: &MemoryStore<Enr>) -> DialOpts {
        let addresses = peer_store
            .addresses_of_peer(peer)
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<Multiaddr>>();
        debug!(?peer, ?addresses, "Let's dial!");
        DialOpts::peer_id(*peer)
            .condition(PeerCondition::DisconnectedAndNotDialing)
            .addresses(addresses)
            .build()
    }
}
