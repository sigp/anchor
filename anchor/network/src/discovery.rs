use std::collections::HashMap;
use std::future::Future;
use std::net::Ipv4Addr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use discv5::enr::{CombinedKey, NodeId};
use discv5::libp2p_identity::{Keypair, PeerId};
use discv5::multiaddr::Multiaddr;
use discv5::{Discv5, Enr};
use futures::stream::FuturesUnordered;
use futures::FutureExt;
use futures::{StreamExt, TryFutureExt};
use libp2p::core::transport::PortUse;
use libp2p::core::Endpoint;
use libp2p::swarm::dummy::ConnectionHandler;
use libp2p::swarm::{
    ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
    THandlerOutEvent, ToSwarm,
};
use lighthouse_network::discovery::enr_ext::{QUIC6_ENR_KEY, QUIC_ENR_KEY};
use lighthouse_network::discovery::DiscoveredPeers;
use lighthouse_network::{CombinedKeyExt, Subnet};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use lighthouse_network::EnrExt;

use crate::Config;

/// The number of closest peers to search for when doing a regular peer search.
///
/// We could reduce this constant to speed up queries however at the cost of security. It will
/// make it easier to peers to eclipse this node. Kademlia suggests a value of 16.
pub const FIND_NODE_QUERY_CLOSEST_PEERS: usize = 16;

#[derive(Debug, Clone, PartialEq)]
struct SubnetQuery {
    subnet: Subnet,
    min_ttl: Option<Instant>,
    retries: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum QueryType {
    /// We are searching for subnet peers.
    Subnet(Vec<SubnetQuery>),
    /// We are searching for more peers without ENR or time constraints.
    FindPeers,
}

/// The result of a query.
struct QueryResult {
    query_type: QueryType,
    result: Result<Vec<Enr>, discv5::QueryError>,
}

// Awaiting the event stream future
enum EventStream {
    /// Awaiting an event stream to be generated. This is required due to the poll nature of
    /// `Discovery`
    Awaiting(
        Pin<Box<dyn Future<Output = Result<mpsc::Receiver<discv5::Event>, discv5::Error>> + Send>>,
    ),
    /// The future has completed.
    Present(mpsc::Receiver<discv5::Event>),
    // The future has failed or discv5 has been disabled. There are no events from discv5.
    InActive,
}

pub struct Discovery {
    /// The handle for the underlying discv5 Server.
    ///
    /// This is behind a Reference counter to allow for futures to be spawned and polled with a
    /// static lifetime.
    discv5: Discv5,

    /// Indicates if we are actively searching for peers. We only allow a single FindPeers query at
    /// a time, regardless of the query concurrency.
    find_peer_active: bool,

    /// Active discovery queries.
    active_queries: FuturesUnordered<std::pin::Pin<Box<dyn Future<Output = QueryResult> + Send>>>,

    /// The discv5 event stream.
    event_stream: EventStream,

    /// Indicates if the discovery service has been started. When the service is disabled, this is
    /// always false.
    pub started: bool,
}

impl Discovery {
    pub async fn new(local_keypair: Keypair, network_config: &Config) -> Result<Self, String> {
        let _enr_dir = match network_config.network_dir.to_str() {
            Some(path) => String::from(path),
            None => String::from(""),
        };

        // TODO handle local enr

        let discv5_listen_config =
            discv5::ListenConfig::from_ip(Ipv4Addr::UNSPECIFIED.into(), 9000);

        // discv5 configuration
        let discv5_config = discv5::ConfigBuilder::new(discv5_listen_config).build();

        // convert the keypair into an ENR key
        let enr_key: CombinedKey = CombinedKey::from_libp2p(local_keypair)?;

        let enr = build_enr(&enr_key, network_config).unwrap();
        let mut discv5 = Discv5::new(enr, enr_key, discv5_config)
            .map_err(|e| format!("Discv5 service failed. Error: {:?}", e))?;

        // Add bootnodes to routing table
        for bootnode_enr in network_config.boot_nodes_enr.clone() {
            // TODO if bootnode_enr.node_id() == local_node_id {
            //     // If we are a boot node, ignore adding it to the routing table
            //     continue;
            // }
            debug!(
                node_id = %bootnode_enr.node_id(),
                peer_id = %bootnode_enr.peer_id(),
                ip = ?bootnode_enr.ip4(),
                udp = ?bootnode_enr.udp4(),
                tcp = ?bootnode_enr.tcp4(),
                quic = ?bootnode_enr.quic4(),
                "Adding node to routing table",
            );

            let repr = bootnode_enr.to_string();
            if let Err(e) = discv5.add_enr(bootnode_enr) {
                error!(
                    addr = repr,
                    error = e.to_string(),
                    "Could not add peer to the local routing table"
                )
            };
        }

        // Start the discv5 service and obtain an event stream
        let event_stream = if !network_config.disable_discovery {
            discv5.start().map_err(|e| e.to_string()).await?;
            debug!("Discovery service started");
            EventStream::Awaiting(Box::pin(discv5.event_stream()))
        } else {
            EventStream::InActive
        };

        if !network_config.boot_nodes_multiaddr.is_empty() {
            // TODO info!(log, "Contacting Multiaddr boot-nodes for their ENR");
        }

        // get futures for requesting the Enrs associated to these multiaddr and wait for their
        // completion
        let mut fut_coll = network_config
            .boot_nodes_multiaddr
            .iter()
            .map(|addr| addr.to_string())
            // request the ENR for this multiaddr and keep the original for logging
            .map(|addr| {
                futures::future::join(
                    discv5.request_enr(addr.clone()),
                    futures::future::ready(addr),
                )
            })
            .collect::<FuturesUnordered<_>>();

        while let Some((result, original_addr)) = fut_coll.next().await {
            match result {
                Ok(enr) => {
                    debug!(
                        node_id = %enr.node_id(),
                        peer_id = %enr.peer_id(),
                        ip = ?enr.ip4(),
                        udp = ?enr.udp4(),
                        tcp = ?enr.tcp4(),
                        quic = ?enr.quic4(),
                         "Adding node to routing table"
                    );
                    let _ = discv5.add_enr(enr).map_err(|e| {
                        error!(
                            addr = original_addr.to_string(),
                            error = e.to_string(),
                            "Could not add peer to the local routing table"
                        )
                    });
                }
                Err(e) => {
                    error!(
                        multiaddr = original_addr.to_string(),
                        error = e.to_string(),
                        "Error getting mapping to ENR"
                    )
                }
            }
        }

        // TODO let update_ports = UpdatePorts {
        //     tcp4: config.enr_tcp4_port.is_none(),
        //     tcp6: config.enr_tcp6_port.is_none(),
        //     quic4: config.enr_quic4_port.is_none(),
        //     quic6: config.enr_quic6_port.is_none(),
        // };

        Ok(Self {
            // cached_enrs: LruCache::new(ENR_CACHE_CAPACITY),
            // network_globals,
            find_peer_active: false,
            // queued_queries: VecDeque::with_capacity(10),
            active_queries: FuturesUnordered::new(),
            discv5,
            event_stream,
            started: !network_config.disable_discovery,
            // update_ports,
            // log,
            // enr_dir,
            // spec: Arc::new(spec.clone()),
        })
    }

    /// This adds a new `FindPeers` query to the queue if one doesn't already exist.
    /// The `target_peers` parameter informs discovery to end the query once the target is found.
    /// The maximum this can be is 16.
    pub fn discover_peers(&mut self, target_peers: usize) {
        // If the discv5 service isn't running or we are in the process of a query, don't bother queuing a new one.
        if !self.started || self.find_peer_active {
            return;
        }
        // Immediately start a FindNode query
        let target_peers = std::cmp::min(FIND_NODE_QUERY_CLOSEST_PEERS, target_peers);
        // TODO debug!(self.log, "Starting a peer discovery request"; "target_peers" => target_peers );
        self.find_peer_active = true;
        self.start_query(QueryType::FindPeers, target_peers, |_| true);
    }

    /// Search for a specified number of new peers using the underlying discovery mechanism.
    ///
    /// This can optionally search for peers for a given predicate. Regardless of the predicate
    /// given, this will only search for peers on the same enr_fork_id as specified in the local
    /// ENR.
    fn start_query(
        &mut self,
        query: QueryType,
        target_peers: usize,
        _additional_predicate: impl Fn(&Enr) -> bool + Send + 'static,
    ) {
        // let enr_fork_id = match self.local_enr().eth2() {
        //     Ok(v) => v,
        //     Err(e) => {
        //         // TODO crit!(self.log, "Local ENR has no fork id"; "error" => e);
        //         return;
        //     }
        // };

        // predicate for finding ssv nodes with a valid tcp port
        let ssv_node_predicate = move |enr: &Enr| {
            if let Some(Ok(is_ssv)) = enr.get_decodable("ssv") {
                is_ssv && enr.tcp4().is_some() || enr.tcp6().is_some()
            } else {
                false
            }
        };

        // General predicate
        let predicate: Box<dyn Fn(&Enr) -> bool + Send> =
            //Box::new(move |enr: &Enr| eth2_fork_predicate(enr) && additional_predicate(enr));
            Box::new(move |enr: &Enr| ssv_node_predicate(enr));

        // Build the future
        let query_future = self
            .discv5
            // Generate a random target node id.
            .find_node_predicate(NodeId::random(), predicate, target_peers)
            .map(|v| QueryResult {
                query_type: query,
                result: v,
            });

        // Add the future to active queries, to be executed.
        self.active_queries.push(Box::pin(query_future));
    }

    /// Process the completed QueryResult returned from discv5.
    fn process_completed_queries(
        &mut self,
        query: QueryResult,
    ) -> Option<HashMap<Enr, Option<Instant>>> {
        match query.query_type {
            QueryType::FindPeers => {
                self.find_peer_active = false;
                match query.result {
                    Ok(r) if r.is_empty() => {
                        debug!("Discovery query yielded no results.");
                    }
                    Ok(r) => {
                        debug!(peers_found = r.len(), "Discovery query completed");
                        let results = r
                            .into_iter()
                            .map(|enr| {
                                // cache the found ENR's
                                //self.cached_enrs.put(enr.peer_id(), enr.clone());
                                (enr, None)
                            })
                            .collect();
                        return Some(results);
                    }
                    Err(e) => {
                        warn!(error = %e, "Discovery query failed");
                    }
                }
            }
            _ => {
                // TODO handle subnet queries
            }
        }
        None
    }

    /// Drives the queries returning any results from completed queries.
    fn poll_queries(&mut self, cx: &mut Context) -> Option<HashMap<Enr, Option<Instant>>> {
        while let Poll::Ready(Some(query_result)) = self.active_queries.poll_next_unpin(cx) {
            let result = self.process_completed_queries(query_result);
            if result.is_some() {
                return result;
            }
        }
        None
    }
}

impl NetworkBehaviour for Discovery {
    // Discovery is not a real NetworkBehaviour...
    type ConnectionHandler = ConnectionHandler;
    type ToSwarm = DiscoveredPeers;

    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(ConnectionHandler)
    }

    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _addr: &Multiaddr,
        _role_override: Endpoint,
        _port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(ConnectionHandler)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            FromSwarm::ConnectionEstablished(c) => {
                debug!("Connection established: {:?}", c);
            }
            _ => {
                // TODO handle other events
            }
        }
    }

    fn on_connection_handler_event(
        &mut self,
        _peer_id: PeerId,
        _connection_id: ConnectionId,
        _event: THandlerOutEvent<Self>,
    ) {
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        if !self.started {
            return Poll::Pending;
        }

        // Process the query queue
        //self.process_queue();

        // Drive the queries and return any results from completed queries
        if let Some(peers) = self.poll_queries(cx) {
            // return the result to the peer manager
            return Poll::Ready(ToSwarm::GenerateEvent(DiscoveredPeers { peers }));
        }
        Poll::Pending
    }
}

/// Builds a anchor ENR given a `network::Config`.
pub fn build_enr(enr_key: &CombinedKey, config: &Config) -> Result<Enr, String> {
    let mut builder = discv5::enr::Enr::builder();
    let (maybe_ipv4_address, maybe_ipv6_address) = &config.enr_address;

    if let Some(ip) = maybe_ipv4_address {
        builder.ip4(*ip);
    }

    if let Some(ip) = maybe_ipv6_address {
        builder.ip6(*ip);
    }

    if let Some(udp4_port) = config.enr_udp4_port {
        builder.udp4(udp4_port.get());
    }

    if let Some(udp6_port) = config.enr_udp6_port {
        builder.udp6(udp6_port.get());
    }

    // Add QUIC fields to the ENR.
    // Since QUIC is used as an alternative transport for the libp2p protocols,
    // the related fields should only be added when both QUIC and libp2p are enabled
    if !config.disable_quic_support {
        // If we are listening on ipv4, add the quic ipv4 port.
        if let Some(quic4_port) = config.enr_quic4_port.or_else(|| {
            config
                .listen_addresses
                .v4()
                .and_then(|v4_addr| v4_addr.quic_port.try_into().ok())
        }) {
            builder.add_value(QUIC_ENR_KEY, &quic4_port.get());
        }

        // If we are listening on ipv6, add the quic ipv6 port.
        if let Some(quic6_port) = config.enr_quic6_port.or_else(|| {
            config
                .listen_addresses
                .v6()
                .and_then(|v6_addr| v6_addr.quic_port.try_into().ok())
        }) {
            builder.add_value(QUIC6_ENR_KEY, &quic6_port.get());
        }
    }

    // If the ENR port is not set, and we are listening over that ip version, use the listening port instead.
    let tcp4_port = config.enr_tcp4_port.or_else(|| {
        config
            .listen_addresses
            .v4()
            .and_then(|v4_addr| v4_addr.tcp_port.try_into().ok())
    });
    if let Some(tcp4_port) = tcp4_port {
        builder.tcp4(tcp4_port.get());
    }

    let tcp6_port = config.enr_tcp6_port.or_else(|| {
        config
            .listen_addresses
            .v6()
            .and_then(|v6_addr| v6_addr.tcp_port.try_into().ok())
    });
    if let Some(tcp6_port) = tcp6_port {
        builder.tcp6(tcp6_port.get());
    }

    builder
        .build(enr_key)
        .map_err(|e| format!("Could not build Local ENR: {:?}", e))
}
