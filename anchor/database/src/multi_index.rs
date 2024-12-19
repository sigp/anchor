use dashmap::DashMap;
use std::{hash::Hash, marker::PhantomData};

/// Marker trait for uniquely identifying indicies
pub trait Unique {}

/// Marker trait for non-uniquely identifiying indicies
pub trait NotUnique {}

/// Index type markers
pub enum Primary {}
pub enum Secondary {}
pub enum Tertiary {}

// Type tags markers
#[derive(Debug)]
pub enum UniqueTag {}
impl Unique for UniqueTag {}

#[derive(Debug)]
pub enum NonUniqueTag {}
impl NotUnique for NonUniqueTag {}

/// Trait for accessing values through a unique index
pub trait UniqueIndex<K, V, I> {
    fn get_by(&self, key: &K) -> Option<V>;
}

/// Trait for accessing values through a non-unique index
pub trait NonUniqueIndex<K, V, I> {
    fn get_all_by(&self, key: &K) -> Option<Vec<V>>;
}

#[derive(Debug, Default)]
struct InnerMaps<K1, K2, K3, V>
where
    K1: Eq + Hash,
    K2: Eq + Hash,
    K3: Eq + Hash,
{
    primary: DashMap<K1, V>,
    secondary_unique: DashMap<K2, K1>,
    secondary_multi: DashMap<K2, Vec<K1>>,
    tertiary_unique: DashMap<K3, K1>,
    tertiary_multi: DashMap<K3, Vec<K1>>,
}

/// A concurrent multi-index map that supports up to three different access patterns.
/// The core differentiates between unique identification and non unique identification. The primary
/// index is forced to always uniquely identify the value. The secondary and tertiary indicies have
/// more flexibility. They key may non uniquely identify many different values, or uniquely identify
/// a single value
///
/// Example: A share is uniquely identified by the Validators public key that it belongs too. A
/// ClusterId does not uniquely identify a share as a cluster contains multiple shares
///
/// - K1: Primary key type (always unique)
/// - K2: Secondary key type
/// - K3: Tertiary key type
/// - V: Value type
/// - U1: Secondary index uniqueness (Unique or NotUnique)
/// - U2: Tertiary index uniqueness (Unique or NotUnique)
#[derive(Debug, Default)]
pub struct MultiIndexMap<K1, K2, K3, V, U1, U2>
where
    K1: Eq + Hash,
    K2: Eq + Hash,
    K3: Eq + Hash,
{
    maps: InnerMaps<K1, K2, K3, V>,
    _marker: PhantomData<(U1, U2)>,
}

impl<K1, K2, K3, V, U1, U2> MultiIndexMap<K1, K2, K3, V, U1, U2>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    V: Clone,
    U1: 'static,
    U2: 'static,
{
    /// Creates a new empty MultiIndexMap
    pub fn new() -> Self {
        Self {
            maps: InnerMaps {
                primary: DashMap::new(),
                secondary_unique: DashMap::new(),
                secondary_multi: DashMap::new(),
                tertiary_unique: DashMap::new(),
                tertiary_multi: DashMap::new(),
            },
            _marker: PhantomData,
        }
    }

    /// Insert a new value and associated keys into the map
    pub fn insert(&self, k1: &K1, k2: &K2, k3: &K3, v: V) {
        // Insert into primary map first
        self.maps.primary.insert(k1.clone(), v);

        // Handle secondary index based on uniqueness
        if std::any::TypeId::of::<U1>() == std::any::TypeId::of::<UniqueTag>() {
            self.maps.secondary_unique.insert(k2.clone(), k1.clone());
        } else {
            self.maps
                .secondary_multi
                .entry(k2.clone())
                .and_modify(|v| v.push(k1.clone()))
                .or_insert_with(|| vec![k1.clone()]);
        }

        // Handle tertiary index based on uniqueness
        if std::any::TypeId::of::<U2>() == std::any::TypeId::of::<UniqueTag>() {
            self.maps.tertiary_unique.insert(k3.clone(), k1.clone());
        } else {
            self.maps
                .tertiary_multi
                .entry(k3.clone())
                .and_modify(|v| v.push(k1.clone()))
                .or_insert_with(|| vec![k1.clone()]);
        }
    }
}

// Implement unique access for primary key
impl<K1, K2, K3, V, U1, U2> UniqueIndex<K1, V, Primary> for MultiIndexMap<K1, K2, K3, V, U1, U2>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    V: Clone,
{
    fn get_by(&self, key: &K1) -> Option<V> {
        self.maps.primary.get(key).map(|v| v.value().clone())
    }
}

// Implement unique access for secondary key
impl<K1, K2, K3, V, U1, U2> UniqueIndex<K2, V, Secondary> for MultiIndexMap<K1, K2, K3, V, U1, U2>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    V: Clone,
    U1: Unique,
{
    fn get_by(&self, key: &K2) -> Option<V> {
        let primary_key = self.maps.secondary_unique.get(key)?;
        self.maps
            .primary
            .get(primary_key.value())
            .map(|v| v.value().clone())
    }
}

// Implement non-unique access for secondary key
impl<K1, K2, K3, V, U1, U2> NonUniqueIndex<K2, V, Secondary>
    for MultiIndexMap<K1, K2, K3, V, U1, U2>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    V: Clone,
    U1: NotUnique,
{
    fn get_all_by(&self, key: &K2) -> Option<Vec<V>> {
        self.maps.secondary_multi.get(key).map(|keys| {
            keys.value()
                .iter()
                .filter_map(|k1| self.maps.primary.get(k1).map(|v| v.value().clone()))
                .collect()
        })
    }
}

// Implement unique access for tertiary key
impl<K1, K2, K3, V, U1, U2> UniqueIndex<K3, V, Tertiary> for MultiIndexMap<K1, K2, K3, V, U1, U2>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    V: Clone,
    U2: Unique,
{
    fn get_by(&self, key: &K3) -> Option<V> {
        let primary_key = self.maps.tertiary_unique.get(key)?;
        self.maps
            .primary
            .get(primary_key.value())
            .map(|v| v.value().clone())
    }
}

// Implement non-unique access for tertiary key
impl<K1, K2, K3, V, U1, U2> NonUniqueIndex<K3, V, Tertiary> for MultiIndexMap<K1, K2, K3, V, U1, U2>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    V: Clone,
    U2: NotUnique,
{
    fn get_all_by(&self, key: &K3) -> Option<Vec<V>> {
        self.maps.tertiary_multi.get(key).map(|keys| {
            keys.value()
                .iter()
                .filter_map(|k1| self.maps.primary.get(k1).map(|v| v.value().clone()))
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_prelude::generators;
    use ssv_types::{Cluster, ClusterId, OperatorId, Share};
    use types::{Address, PublicKey};

    #[test]
    fn test_nonunique() {
        let cluster_id = ClusterId(10);
        let operator_id = OperatorId(10);
        let owner = Address::random();

        // Shares with different public keys, but same cluster id and owner
        let share_1 = generators::share::random(cluster_id, operator_id);
        let pk_1 = generators::pubkey::random();
        let share_2 = generators::share::random(cluster_id, operator_id);
        let pk_2 = generators::pubkey::random();

        // A MultiIndexMap for accessing Shares
        // Primary Key: validator public key which uniquly identifies a share
        // Secondary Key: cluster id which does not uniquely identify a share (NonUniqueTag)
        // Tertiary Key: owner address which does not uniquely identify a share (NonUniqueTag)
        let map: MultiIndexMap<PublicKey, ClusterId, Address, Share, NonUniqueTag, NonUniqueTag> =
            MultiIndexMap::new();

        // insert the data
        map.insert(&pk_1, &cluster_id, &owner, share_1);
        map.insert(&pk_2, &cluster_id, &owner, share_2);

        // This does not compile since
        // let shares = map.get_all_by(&pk_1);

        // This does compile
        let share_1 = map.get_by(&pk_1);
        assert!(share_1.is_some());

        // This does not compile since we enforce NonUnique via NonUniqueTag
        // let share = map.get_by(&cluster_id);

        // This does compile
        let shares = map.get_all_by(&cluster_id).expect("Failed to get shares");
        assert!(shares.len() == 2);

        // Like above, this does not compile
        // let share = map.get_by(&owner);

        // This does compile
        let shares = map.get_all_by(&owner).expect("Failed to get shares");
        assert!(shares.len() == 2);
    }

    #[test]
    fn test_unique() {
        // generate a cluster and its corresponding validator
        let cluster = generators::cluster::random(4);
        let validator_metadata = generators::validator::random_metadata(cluster.cluster_id);

        // A MultiIndexMap for accessing a cluster
        // Primary Key: cluster id that uniquely identifies the cluster
        // Secondary Key: validator public key that uniquely identifies this cluster
        // Tertiary Key: owner address that uniquely identifies this cluster
        let map: MultiIndexMap<ClusterId, PublicKey, Address, Cluster, UniqueTag, UniqueTag> =
            MultiIndexMap::new();

        // insert the cluster
        map.insert(
            &cluster.cluster_id,
            &validator_metadata.public_key,
            &cluster.owner,
            cluster.clone(),
        );

        // - Fetch via cluster id
        // This does not compile
        //let cluster  = map.get_all_by(&cluster.cluster_id);
        // This does compile
        let c = map.get_by(&cluster.cluster_id);
        assert!(c.is_some());

        // - Fetch via public key
        // This does not compile
        //let cluster = map.get_all_by(&validator_metadata.public_key);
        // This does compile due to UniqueTag
        let c = map.get_by(&validator_metadata.public_key);
        assert!(c.is_some());

        // - Fetch via owner
        // This does not compile
        //let cluster = map.get_all_by(&cluster.owner);
        // This does compile due to UniqueTag
        let c = map.get_by(&cluster.owner);
        assert!(c.is_some());
    }
}
