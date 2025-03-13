use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;

/// Marker trait for uniquely identifying indices
pub trait Unique {}

/// Marker trait for non-uniquely identifying indices
pub trait NotUnique {}

/// Index type markers
pub enum Primary {}
pub enum Secondary {}
pub enum Tertiary {}
pub enum Quaternary {}

/// Type tags markers
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

/// Inner storage maps for the multi-index map, now supporting a quaternary index.
/// - K1: Primary key type (always unique)
/// - K2: Secondary key type
/// - K3: Tertiary key type
/// - K4: Quaternary key type
/// - V: Value type
#[derive(Debug)]
struct InnerMaps<K1, K2, K3, K4, V>
where
    K1: Eq + Hash,
    K2: Eq + Hash,
    K3: Eq + Hash,
    K4: Eq + Hash,
{
    primary: HashMap<K1, V>,
    secondary_unique: HashMap<K2, K1>,
    secondary_multi: HashMap<K2, Vec<K1>>,
    tertiary_unique: HashMap<K3, K1>,
    tertiary_multi: HashMap<K3, Vec<K1>>,
    quaternary_unique: HashMap<K4, K1>,
    quaternary_multi: HashMap<K4, Vec<K1>>,
}

/// A concurrent multi-index map that supports up to four different access patterns.
/// The core differentiates between unique identification and non-unique identification.
/// The primary index is forced to always uniquely identify the value. The secondary, tertiary,
/// and quaternary indices have more flexibility. A key may non-uniquely identify many values,
/// or uniquely identify a single value.
///
/// Example: A share might be uniquely identified by a primary key (like a Validators public key)
/// while a secondary or tertiary index (like a ClusterId) does not uniquely identify a share. The
/// new quaternary index provides an additional access pattern.
///
/// - K1: Primary key type (always unique)
/// - K2: Secondary key type
/// - K3: Tertiary key type
/// - K4: Quaternary key type
/// - V: Value type
/// - U1: Secondary index uniqueness (Unique or NotUnique)
/// - U2: Tertiary index uniqueness (Unique or NotUnique)
/// - U3: Quaternary index uniqueness (Unique or NotUnique)
#[derive(Debug)]
pub struct MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash,
    K2: Eq + Hash,
    K3: Eq + Hash,
    K4: Eq + Hash,
{
    maps: InnerMaps<K1, K2, K3, K4, V>,
    _marker: PhantomData<(U1, U2, U3)>,
}

impl<K1, K2, K3, K4, V, U1, U2, U3> Default for MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
    U1: 'static,
    U2: 'static,
    U3: 'static,
{
    fn default() -> Self {
        Self {
            maps: InnerMaps {
                primary: HashMap::new(),
                secondary_unique: HashMap::new(),
                secondary_multi: HashMap::new(),
                tertiary_unique: HashMap::new(),
                tertiary_multi: HashMap::new(),
                quaternary_unique: HashMap::new(),
                quaternary_multi: HashMap::new(),
            },
            _marker: PhantomData,
        }
    }
}

impl<K1, K2, K3, K4, V, U1, U2, U3> MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
    U1: 'static,
    U2: 'static,
    U3: 'static,
{
    /// Creates a new empty MultiIndexMap.
    pub fn new() -> Self {
        Self {
            maps: InnerMaps {
                primary: HashMap::new(),
                secondary_unique: HashMap::new(),
                secondary_multi: HashMap::new(),
                tertiary_unique: HashMap::new(),
                tertiary_multi: HashMap::new(),
                quaternary_unique: HashMap::new(),
                quaternary_multi: HashMap::new(),
            },
            _marker: PhantomData,
        }
    }

    /// Returns the number of entries in the primary map.
    pub fn length(&self) -> usize {
        self.maps.primary.len()
    }

    /// Inserts a new value and associated keys into the map.
    /// Inserts the primary key and value first, then updates the secondary, tertiary,
    /// and quaternary indices based on their uniqueness.
    pub fn insert(&mut self, k1: &K1, k2: &K2, k3: &K3, k4: &K4, v: V) {
        // Insert into primary map first
        self.maps.primary.insert(k1.clone(), v);

        // Handle secondary index based on uniqueness
        if std::any::TypeId::of::<U1>() == std::any::TypeId::of::<UniqueTag>() {
            self.maps.secondary_unique.insert(k2.clone(), k1.clone());
        } else {
            self.maps
                .secondary_multi
                .entry(k2.clone())
                .and_modify(|vec| vec.push(k1.clone()))
                .or_insert_with(|| vec![k1.clone()]);
        }

        // Handle tertiary index based on uniqueness
        if std::any::TypeId::of::<U2>() == std::any::TypeId::of::<UniqueTag>() {
            self.maps.tertiary_unique.insert(k3.clone(), k1.clone());
        } else {
            self.maps
                .tertiary_multi
                .entry(k3.clone())
                .and_modify(|vec| vec.push(k1.clone()))
                .or_insert_with(|| vec![k1.clone()]);
        }

        // Handle quaternary index based on uniqueness
        if std::any::TypeId::of::<U3>() == std::any::TypeId::of::<UniqueTag>() {
            self.maps.quaternary_unique.insert(k4.clone(), k1.clone());
        } else {
            self.maps
                .quaternary_multi
                .entry(k4.clone())
                .and_modify(|vec| vec.push(k1.clone()))
                .or_insert_with(|| vec![k1.clone()]);
        }
    }

    /// Removes a value and all its indexes using the primary key.
    pub fn remove(&mut self, k1: &K1) -> Option<V> {
        // Remove from primary storage
        let removed = self.maps.primary.remove(k1)?;

        // Remove from secondary index
        if std::any::TypeId::of::<U1>() == std::any::TypeId::of::<UniqueTag>() {
            // For unique indexes, just remove the entry that points to this k1
            self.maps.secondary_unique.retain(|_, v| v != k1);
        } else {
            // For non-unique indexes, remove k1 from any vectors it appears in
            self.maps.secondary_multi.retain(|_, vec| {
                vec.retain(|x| x != k1);
                !vec.is_empty()
            });
        }

        // Remove from tertiary index
        if std::any::TypeId::of::<U2>() == std::any::TypeId::of::<UniqueTag>() {
            // For unique indexes, just remove the entry that points to this k1
            self.maps.tertiary_unique.retain(|_, v| v != k1);
        } else {
            // For non-unique indexes, remove k1 from any vectors it appears in
            self.maps.tertiary_multi.retain(|_, vec| {
                vec.retain(|x| x != k1);
                !vec.is_empty()
            });
        }

        // Remove from quaternary index
        if std::any::TypeId::of::<U3>() == std::any::TypeId::of::<UniqueTag>() {
            self.maps.quaternary_unique.retain(|_, v| v != k1);
        } else {
            self.maps.quaternary_multi.retain(|_, vec| {
                vec.retain(|x| x != k1);
                !vec.is_empty()
            });
        }

        Some(removed)
    }

    /// Updates an existing value using the primary key.
    /// Only updates if the primary key exists; indexes remain unchanged.
    pub fn update(&mut self, k1: &K1, new_value: V) -> Option<V> {
        if !self.maps.primary.contains_key(k1) {
            return None;
        }

        // Only update the value in primary storage
        self.maps.primary.insert(k1.clone(), new_value)
    }
}

// Implement unique access for primary key.
impl<K1, K2, K3, K4, V, U1, U2, U3> UniqueIndex<K1, V, Primary>
    for MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
{
    fn get_by(&self, key: &K1) -> Option<V> {
        self.maps.primary.get(key).cloned()
    }
}

// Implement unique access for secondary key.
impl<K1, K2, K3, K4, V, U1, U2, U3> UniqueIndex<K2, V, Secondary>
    for MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
    U1: Unique,
{
    fn get_by(&self, key: &K2) -> Option<V> {
        let primary_key = self.maps.secondary_unique.get(key)?;
        self.maps.primary.get(primary_key).cloned()
    }
}

// Implement non-unique access for secondary key.
impl<K1, K2, K3, K4, V, U1, U2, U3> NonUniqueIndex<K2, V, Secondary>
    for MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
    U1: NotUnique,
{
    fn get_all_by(&self, key: &K2) -> Option<Vec<V>> {
        self.maps.secondary_multi.get(key).map(|keys| {
            keys.iter()
                .filter_map(|k1| self.maps.primary.get(k1).cloned())
                .collect()
        })
    }
}

// Implement unique access for tertiary key.
impl<K1, K2, K3, K4, V, U1, U2, U3> UniqueIndex<K3, V, Tertiary>
    for MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
    U2: Unique,
{
    fn get_by(&self, key: &K3) -> Option<V> {
        let primary_key = self.maps.tertiary_unique.get(key)?;
        self.maps.primary.get(primary_key).cloned()
    }
}

// Implement non-unique access for tertiary key.
impl<K1, K2, K3, K4, V, U1, U2, U3> NonUniqueIndex<K3, V, Tertiary>
    for MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
    U2: NotUnique,
{
    fn get_all_by(&self, key: &K3) -> Option<Vec<V>> {
        self.maps.tertiary_multi.get(key).map(|keys| {
            keys.iter()
                .filter_map(|k1| self.maps.primary.get(k1).cloned())
                .collect()
        })
    }
}

// Implement unique access for quaternary key.
impl<K1, K2, K3, K4, V, U1, U2, U3> UniqueIndex<K4, V, Quaternary>
    for MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
    U3: Unique,
{
    fn get_by(&self, key: &K4) -> Option<V> {
        let primary_key = self.maps.quaternary_unique.get(key)?;
        self.maps.primary.get(primary_key).cloned()
    }
}

// Implement non-unique access for quaternary key.
impl<K1, K2, K3, K4, V, U1, U2, U3> NonUniqueIndex<K4, V, Quaternary>
    for MultiIndexMap<K1, K2, K3, K4, V, U1, U2, U3>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone,
    K3: Eq + Hash + Clone,
    K4: Eq + Hash + Clone,
    V: Clone,
    U3: NotUnique,
{
    fn get_all_by(&self, key: &K4) -> Option<Vec<V>> {
        self.maps.quaternary_multi.get(key).map(|keys| {
            keys.iter()
                .filter_map(|k1| self.maps.primary.get(k1).cloned())
                .collect()
        })
    }
}

#[cfg(test)]
mod multi_index_tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct TestValue {
        id: i32,
        data: String,
    }

    #[test]
    fn test_basic_operations() {
        // Using unique indices for all secondary, tertiary, and quaternary keys.
        let mut map: MultiIndexMap<
            i32,
            String,
            bool,
            char,
            TestValue,
            UniqueTag,
            UniqueTag,
            UniqueTag,
        > = MultiIndexMap::new();

        let value = TestValue {
            id: 1,
            data: "test".to_string(),
        };

        // Test insertion with quaternary key 'a'
        map.insert(&1, &"key1".to_string(), &true, &'a', value.clone());

        // Test primary key access
        assert_eq!(map.get_by(&1), Some(value.clone()));

        // Test secondary key access
        assert_eq!(map.get_by(&"key1".to_string()), Some(value.clone()));

        // Test tertiary key access
        assert_eq!(map.get_by(&true), Some(value.clone()));

        // Test quaternary key access
        assert_eq!(map.get_by(&'a'), Some(value.clone()));

        // Test update
        let new_value = TestValue {
            id: 1,
            data: "updated".to_string(),
        };
        map.update(&1, new_value.clone());
        assert_eq!(map.get_by(&1), Some(new_value.clone()));

        // Test removal: all indices should be cleaned up
        assert_eq!(map.remove(&1), Some(new_value.clone()));
        assert_eq!(map.get_by(&1), None);
        assert_eq!(map.get_by(&"key1".to_string()), None);
        assert_eq!(map.get_by(&true), None);
        assert_eq!(map.get_by(&'a'), None);
    }

    #[test]
    fn test_non_unique_indices() {
        // Using non-unique indices for all secondary, tertiary, and quaternary keys.
        let mut map: MultiIndexMap<
            i32,
            String,
            bool,
            char,
            TestValue,
            NonUniqueTag,
            NonUniqueTag,
            NonUniqueTag,
        > = MultiIndexMap::new();

        let value1 = TestValue {
            id: 1,
            data: "test1".to_string(),
        };
        let value2 = TestValue {
            id: 2,
            data: "test2".to_string(),
        };

        // Insert multiple values with same secondary, tertiary, and quaternary keys.
        map.insert(&1, &"shared_key".to_string(), &true, &'z', value1.clone());
        map.insert(&2, &"shared_key".to_string(), &true, &'z', value2.clone());

        // Test primary key access (still unique)
        assert_eq!(map.get_by(&1), Some(value1.clone()));
        assert_eq!(map.get_by(&2), Some(value2.clone()));

        // Test secondary key access (non-unique)
        let secondary_values = map.get_all_by(&"shared_key".to_string()).unwrap();
        assert_eq!(secondary_values.len(), 2);
        assert!(secondary_values.contains(&value1));
        assert!(secondary_values.contains(&value2));

        // Test tertiary key access (non-unique)
        let tertiary_values = map.get_all_by(&true).unwrap();
        assert_eq!(tertiary_values.len(), 2);
        assert!(tertiary_values.contains(&value1));
        assert!(tertiary_values.contains(&value2));

        // Test quaternary key access (non-unique)
        let quaternary_values = map.get_all_by(&'z').unwrap();
        assert_eq!(quaternary_values.len(), 2);
        assert!(quaternary_values.contains(&value1));
        assert!(quaternary_values.contains(&value2));

        // Test removal maintains other entries
        map.remove(&1);
        assert_eq!(map.get_by(&1), None);
        assert_eq!(map.get_by(&2), Some(value2.clone()));

        let remaining_secondary = map.get_all_by(&"shared_key".to_string()).unwrap();
        assert_eq!(remaining_secondary.len(), 1);
        assert_eq!(remaining_secondary[0], value2);
    }

    #[test]
    fn test_mixed_uniqueness() {
        // Mixed: unique secondary, non-unique tertiary, unique quaternary.
        let mut map: MultiIndexMap<
            i32,
            String,
            bool,
            char,
            TestValue,
            UniqueTag,
            NonUniqueTag,
            UniqueTag,
        > = MultiIndexMap::new();

        let value1 = TestValue {
            id: 1,
            data: "test1".to_string(),
        };
        let value2 = TestValue {
            id: 2,
            data: "test2".to_string(),
        };

        // Insert values with unique secondary keys but shared tertiary and different quaternary keys.
        map.insert(&1, &"key1".to_string(), &true, &'q', value1.clone());
        map.insert(&2, &"key2".to_string(), &true, &'r', value2.clone());

        // Test unique secondary key access
        assert_eq!(map.get_by(&"key1".to_string()), Some(value1.clone()));
        assert_eq!(map.get_by(&"key2".to_string()), Some(value2.clone()));

        // Test non-unique tertiary key access
        let tertiary_values = map.get_all_by(&true).unwrap();
        assert_eq!(tertiary_values.len(), 2);
        assert!(tertiary_values.contains(&value1));
        assert!(tertiary_values.contains(&value2));

        // Test unique quaternary key access
        assert_eq!(map.get_by(&'q'), Some(value1.clone()));
        assert_eq!(map.get_by(&'r'), Some(value2.clone()));
    }

    #[test]
    fn test_empty_cases() {
        let mut map: MultiIndexMap<
            i32,
            String,
            bool,
            char,
            TestValue,
            UniqueTag,
            UniqueTag,
            UniqueTag,
        > = MultiIndexMap::new();

        // Test access on empty map
        assert_eq!(map.get_by(&1), None);
        assert_eq!(map.get_by(&"key".to_string()), None);
        assert_eq!(map.get_by(&true), None);
        assert_eq!(map.get_by(&'x'), None);

        // Test remove on empty map
        assert_eq!(map.remove(&1), None);

        // Test update on empty map
        let value = TestValue {
            id: 1,
            data: "test".to_string(),
        };
        assert_eq!(map.update(&1, value), None);
    }
}
