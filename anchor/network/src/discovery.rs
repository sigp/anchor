use std::{
    fs::{self, File},
    future::Future,
    io::Write,
    net::{SocketAddrV4, SocketAddrV6},
    path::{Path, PathBuf},
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
};

use discv5::{
    Discv5, Enr, ProtocolIdentity,
    enr::{CombinedKey, Error, NodeId},
    libp2p_identity::{Keypair, PeerId},
    multiaddr::Multiaddr,
};
use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use libp2p::{
    bytes::Bytes,
    core::{Endpoint, transport::PortUse},
    swarm::{
        ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler, THandlerInEvent,
        THandlerOutEvent, ToSwarm, dummy,
    },
};
use lighthouse_network::{
    CombinedKeyExt, EnrExt,
    discovery::{
        UpdatePorts,
        enr_ext::{QUIC_ENR_KEY, QUIC6_ENR_KEY},
    },
};
use ssv_types::domain_type::DomainType;
use ssz::{Decode, Encode};
use ssz_types::{BitVector, Bitfield, length::Fixed, typenum::U128};
use subnet_service::SubnetId;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

use crate::{
    Config,
    discovery::DiscoveryError::{Discv5Init, Discv5Start, EnrKey},
};

/// Target number of peers to search for given a grouped subnet query.
const TARGET_PEERS_FOR_GROUPED_QUERY: usize = 6;
/// The number of closest peers to search for when doing a regular peer search.
///
/// We could reduce this constant to speed up queries however at the cost of security. It will
/// make it easier to peers to eclipse this node. Kademlia suggests a value of 16.
pub const FIND_NODE_QUERY_CLOSEST_PEERS: usize = 16;

pub const ENR_FILENAME: &str = "enr.dat";

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("Failed to parse keypair into an ENR key: {0}")]
    EnrKey(String),

    #[error("Failed to build local ENR: {0}")]
    EnrBuild(#[from] Error),

    #[error("Discv5 initialization error: {0}")]
    Discv5Init(String),

    #[error("Discv5 start error: {0}")]
    Discv5Start(discv5::Error),
}

#[derive(Debug, Clone, PartialEq)]
struct SubnetQuery {
    subnet: SubnetId,
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

#[derive(Debug, Clone)]
pub struct DiscoveredPeers {
    pub peers: Vec<Enr>,
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

impl EventStream {
    fn recv(&mut self, cx: &mut Context) -> Option<discv5::Event> {
        if let EventStream::Awaiting(future) = self
            && let Poll::Ready(Ok(receiver)) = future.as_mut().poll(cx)
        {
            *self = EventStream::Present(receiver);
        }

        if let EventStream::Present(receiver) = self
            && let Poll::Ready(Some(event)) = receiver.poll_recv(cx)
        {
            Some(event)
        } else {
            None
        }
    }
}

pub struct ProtocolId {}

impl ProtocolIdentity for ProtocolId {
    const PROTOCOL_ID_BYTES: [u8; 6] = *b"ssvdv5";
    const PROTOCOL_VERSION_BYTES: [u8; 2] = 0x0001_u16.to_be_bytes();
}

pub struct Discovery {
    /// The handle for the underlying discv5 Server.
    ///
    /// This is behind a Reference counter to allow for futures to be spawned and polled with a
    /// static lifetime.
    discv5: Discv5<ProtocolId>,

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

    /// Specifies whether various port numbers should be updated after the discovery service has
    /// been started
    pub update_ports: UpdatePorts,

    domain_type: DomainType,

    enr_file_path: PathBuf,
}

impl Discovery {
    pub async fn new(
        local_keypair: Keypair,
        network_config: &Config,
    ) -> Result<Self, DiscoveryError> {
        let enr_file_path = network_config.network_dir.enr_file();

        let discv5_listen_config = discv5::ListenConfig::from_two_sockets(
            network_config
                .listen_addresses
                .v4()
                .map(|addr| SocketAddrV4::new(addr.addr, addr.disc_port)),
            network_config
                .listen_addresses
                .v6()
                .map(|addr| SocketAddrV6::new(addr.addr, addr.disc_port, 0, 0)),
        );

        // discv5 configuration
        let discv5_config = discv5::ConfigBuilder::new(discv5_listen_config).build();

        // convert the keypair into an ENR key
        let enr_key: CombinedKey =
            CombinedKey::from_libp2p(local_keypair).map_err(|e| EnrKey(e.to_string()))?;

        let previous_enr = load_enr_from_disk(&enr_file_path);
        let enr = build_enr(&enr_key, network_config, previous_enr)?;
        save_enr_to_disk(&enr_file_path, &enr);
        let local_node_id = enr.node_id();

        info!(%enr, "Created local ENR");

        let mut discv5 = Discv5::<ProtocolId>::new(enr, enr_key, discv5_config)
            .map_err(|e| Discv5Init(e.to_string()))?;

        // Add bootnodes to routing table
        for bootnode_enr in network_config.boot_nodes_enr.clone() {
            if bootnode_enr.node_id() == local_node_id {
                // If we are a boot node, ignore adding ourselves to the routing table
                continue;
            }
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
            discv5.start().await.map_err(Discv5Start)?; // can't convert automatically cause discv5::Error does not implement std::error::Error

            debug!("Discovery service started");
            EventStream::Awaiting(Box::pin(discv5.event_stream()))
        } else {
            EventStream::InActive
        };

        if !network_config.boot_nodes_multiaddr.is_empty() {
            info!("Contacting Multiaddr boot-nodes for their ENR");
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

        // Update local ports from libp2p events
        let update_ports = UpdatePorts {
            tcp4: network_config.enr_tcp4_port.is_none(),
            tcp6: network_config.enr_tcp6_port.is_none(),
            quic4: network_config.enr_quic4_port.is_none(),
            quic6: network_config.enr_quic6_port.is_none(),
        };

        Ok(Self {
            find_peer_active: false,
            // queued_queries: VecDeque::with_capacity(10),
            active_queries: FuturesUnordered::new(),
            discv5,
            event_stream,
            started: !network_config.disable_discovery,
            domain_type: network_config.domain_type,
            update_ports,
            enr_file_path,
        })
    }

    /// This adds a new `FindPeers` query to the queue if one doesn't already exist.
    /// The `target_peers` parameter informs discovery to end the query once the target is found.
    /// The maximum this can be is 16.
    pub fn discover_peers(&mut self, target_peers: usize) {
        // If the discv5 service isn't running or we are in the process of a query, don't bother
        // queuing a new one.
        if !self.started || self.find_peer_active {
            return;
        }
        // Immediately start a FindNode query
        let target_peers = std::cmp::min(FIND_NODE_QUERY_CLOSEST_PEERS, target_peers);
        debug!(target_peers, "Starting a peer discovery request");
        self.find_peer_active = true;
        self.start_query(QueryType::FindPeers, target_peers, |_| true);
    }

    /// Runs a discovery request for a given group of subnets.
    pub fn start_subnet_query(&mut self, subnets: Vec<SubnetId>) {
        let subnet_queries = subnets
            .iter()
            .map(|&subnet| SubnetQuery { subnet, retries: 0 })
            .collect();

        self.start_query(
            QueryType::Subnet(subnet_queries),
            TARGET_PEERS_FOR_GROUPED_QUERY,
            subnet_predicate(subnets),
        );
    }

    pub fn set_subscribed(&mut self, subnet: SubnetId, subscribed: bool) {
        let enr = self.discv5.local_enr();

        let mut subnets = committee_bitfield(&enr).unwrap_or_default();

        if let Err(err) = subnets.set(*subnet as usize, subscribed) {
            error!(
                ?err,
                ?subnet,
                "Could not set subnet bit in ENR - invalid subnet?"
            );
        }

        if let Err(err) = self
            .discv5
            .enr_insert::<Bytes>("subnets", &subnets.as_ssz_bytes().into())
        {
            error!(?err, "Unable to update ENR");
        } else {
            debug!(enr=?self.discv5.local_enr(), "Updated subnets in ENR");
            save_enr_to_disk(&self.enr_file_path, &self.discv5.local_enr());
        }
    }

    /// Try to update an ENR port based on port type and configuration.
    ///
    /// This method centralizes all port update logic in one place:
    /// 1. Checks if updates are allowed for this port type
    /// 2. Gets current port value from ENR
    /// 3. Updates the port if needed
    /// 4. Persists changes to disk
    ///
    /// Parameters:
    /// - `is_tcp`: Whether this is a TCP port (true) or QUIC port (false)
    /// - `is_ipv6`: Whether this is an IPv6 port (true) or IPv4 port (false)
    /// - `port`: The new port value to set
    ///
    /// Returns:
    /// - `Ok(true)`: Port was updated and persisted to disk
    /// - `Ok(false)`: No update was needed (config disallows it or port already matches)
    /// - `Err(String)`: Update failed with the given error message
    pub fn try_update_port(
        &mut self,
        is_tcp: bool,
        is_ipv6: bool,
        new_port: u16,
    ) -> Result<bool, String> {
        let (read_fn, key): (fn(&_) -> Option<u16>, &str) = match (is_tcp, is_ipv6) {
            (true, false) if self.update_ports.tcp4 => (Enr::tcp4, "tcp"),
            (true, true) if self.update_ports.tcp6 => (Enr::tcp6, "tcp6"),
            (false, false) if self.update_ports.quic4 => (Enr::quic4, "quic4"),
            (false, true) if self.update_ports.quic6 => (Enr::quic6, "quic6"),
            _ => return Ok(false),
        };
        let port_opt = read_fn(&self.discv5.external_enr().read());

        if port_opt == Some(new_port) {
            return Ok(false);
        }

        self.discv5
            .enr_insert(key, &new_port)
            .map_err(|e| format!("{e:?}"))?;

        save_enr_to_disk(&self.enr_file_path, &self.discv5.local_enr());
        Ok(true)
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
        additional_predicate: impl Fn(&Enr) -> bool + Send + 'static,
    ) {
        // predicate for finding nodes with a valid tcp port
        let tcp_predicate = move |enr: &Enr| enr.tcp4().is_some() || enr.tcp6().is_some();

        // Capture a copy of the domain type so the closure no longer references `self`.
        let local_domain_type = self.domain_type;

        let domain_type_predicate = move |enr: &Enr| {
            if let Some(Ok(domain_type)) = enr.get_decodable::<[u8; 4]>("domaintype") {
                local_domain_type.0 == domain_type
            } else {
                trace!(?enr, "Rejecting ENR with missing domaintype");
                false
            }
        };

        // General predicate
        let predicate: Box<dyn Fn(&Enr) -> bool + Send> = Box::new(move |enr: &Enr| {
            let ok = tcp_predicate(enr) && domain_type_predicate(enr) && additional_predicate(enr);
            trace!(?enr, ok, "considered ENR for discovery");
            ok
        });

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
    fn process_completed_queries(&mut self, query: QueryResult) -> Option<Vec<Enr>> {
        match query.query_type {
            QueryType::FindPeers => {
                self.find_peer_active = false;
                match query.result {
                    Ok(r) if r.is_empty() => {
                        debug!("Discovery query yielded no results.");
                    }
                    Ok(results) => {
                        debug!(peers = ?results, "Discovery query completed");
                        return Some(results);
                    }
                    Err(e) => {
                        warn!(error = %e, "Discovery query failed");
                    }
                }
            }
            QueryType::Subnet(queries) => {
                let subnets_searched_for: Vec<SubnetId> =
                    queries.iter().map(|query| query.subnet).collect();

                match query.result {
                    Ok(r) if r.is_empty() => {
                        debug!(
                            subnets_searched_for = ?subnets_searched_for,
                            "Grouped subnet discovery query yielded no results.",
                        );
                    }
                    Ok(results) => {
                        debug!(
                            peers = ?results,
                            subnets_searched_for = ?subnets_searched_for,
                            "Peer grouped subnet discovery request completed",
                        );

                        return Some(results);
                    }
                    Err(e) => {
                        warn!(error = %e, "Subnet query failed");
                    }
                }
            }
        }
        None
    }

    /// Drives the queries returning any results from completed queries.
    fn poll_queries(&mut self, cx: &mut Context) -> Option<Vec<Enr>> {
        while let Poll::Ready(Some(query_result)) = self.active_queries.poll_next_unpin(cx) {
            let result = self.process_completed_queries(query_result);
            if result.is_some() {
                return result;
            }
        }
        None
    }

    pub fn local_enr(&self) -> Enr {
        self.discv5.local_enr()
    }
}

impl NetworkBehaviour for Discovery {
    // Discovery is not a real NetworkBehaviour...
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = DiscoveredPeers;

    fn handle_established_inbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _local_addr: &Multiaddr,
        _remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(dummy::ConnectionHandler)
    }

    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _addr: &Multiaddr,
        _role_override: Endpoint,
        _port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, _event: FromSwarm) {}

    fn on_connection_handler_event(
        &mut self,
        _peer_id: PeerId,
        _connection_id: ConnectionId,
        _event: THandlerOutEvent<Self>,
    ) {
    }

    #[allow(clippy::single_match)]
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        if !self.started {
            return Poll::Pending;
        }

        // Drive the queries and return any results from completed queries
        if let Some(peers) = self.poll_queries(cx) {
            // return the result to the peer manager
            return Poll::Ready(ToSwarm::GenerateEvent(DiscoveredPeers { peers }));
        }

        while let Some(event) = self.event_stream.recv(cx) {
            if let discv5::Event::SocketUpdated(socket_addr) = event {
                info!(ip = %socket_addr.ip(), udp_port = %socket_addr.port(), "Address updated");

                let was_updated = if socket_addr.is_ipv4() {
                    self.try_update_port(true, false, socket_addr.port())
                } else {
                    self.try_update_port(true, true, socket_addr.port())
                };

                match was_updated {
                    Ok(true) => {
                        info!(ip = %socket_addr.ip(), udp_port = %socket_addr.port(), "ENR port updated")
                    }
                    Ok(false) => {
                        debug!(ip = %socket_addr.ip(), udp_port = %socket_addr.port(), "No ENR port update needed")
                    }
                    Err(e) => warn!(error = e, "Failed to update ENR port"),
                }
            }
        }

        Poll::Pending
    }
}

/// Builds a anchor ENR given a `network::Config`.
pub fn build_enr(
    enr_key: &CombinedKey,
    config: &Config,
    prev_enr: Option<Enr>,
) -> Result<Enr, Error> {
    let mut builder = Enr::builder();

    if let Some(prev_enr) = prev_enr {
        builder.seq(prev_enr.seq() + 1);
    }

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

    // If the ENR port is not set, and we are listening over that ip version, use the listening port
    // instead.
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

    // set the "subnets" field on our ENR
    builder.add_value::<Bytes>("subnets", &BitVector::<U128>::new().as_ssz_bytes().into());

    // set the "domaintype" field on our ENR
    builder.add_value::<[u8; 4]>("domaintype", &config.domain_type.0);

    // finally, set "ssv" to true
    builder.add_value::<bool>("ssv", &true);

    let enr = builder.build(enr_key)?;
    Ok(enr)
}

/// Loads an ENR from disk
pub fn load_enr_from_disk(path: &Path) -> Option<Enr> {
    fs::read_to_string(path)
        .ok()
        .and_then(|enr| Enr::from_str(&enr).ok())
}

/// Saves an ENR to disk
pub fn save_enr_to_disk(path: &Path, enr: &Enr) {
    match File::create(path).and_then(|mut f| f.write_all(enr.to_base64().as_bytes())) {
        Ok(_) => {
            debug!("ENR written to disk");
        }
        Err(e) => {
            warn!(
                file = %path.display(),
                error = %e,
                "Could not write ENR to file"
            );
        }
    }
}

pub fn committee_bitfield(enr: &Enr) -> Result<Bitfield<Fixed<U128>>, &'static str> {
    let bitfield_bytes: Bytes = enr
        .get_decodable("subnets")
        .ok_or("ENR subnet bitfield non-existent")?
        .map_err(|_| "Invalid RLP Encoding")?;

    BitVector::<U128>::from_ssz_bytes(&bitfield_bytes)
        .map_err(|_| "Could not decode the ENR subnets bitfield")
}

/// Returns the predicate for a given subnet.
pub fn subnet_predicate(subnets: Vec<SubnetId>) -> impl Fn(&Enr) -> bool + Send {
    move |enr: &Enr| {
        let committee_bitfield: Bitfield<Fixed<U128>> = match committee_bitfield(enr) {
            Ok(b) => b,
            Err(_e) => return false,
        };

        let predicate = subnets
            .iter()
            .any(|&s| committee_bitfield.get(*s as usize).unwrap_or(false));

        if !predicate {
            trace!(
                peer_id = %enr.peer_id(),
                "Peer found but not on any of the desired subnets",
            );
        }
        predicate
    }
}
