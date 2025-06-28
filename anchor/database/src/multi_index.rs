use std::{collections::HashMap, hash::Hash, marker::PhantomData, sync::Arc};

/// Marker trait for uniquely identifying indices
#[allow(dead_code)]
pub trait Unique {}

/// Marker trait for non-uniquely identifying indices
#[allow(dead_code)]
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

pub trait IndexDescriptor {
    type Key: Eq + Hash + Clone;
    type Uniqueness: 'static;
}

#[derive(Debug)]
pub struct UniqueIndex<K>(PhantomData<K>);

impl<K: Eq + Hash + Clone> IndexDescriptor for UniqueIndex<K> {
    type Key = K;
    type Uniqueness = UniqueTag;
}

#[derive(Debug)]
pub struct NonUniqueIndex<K>(PhantomData<K>);

impl<K: Eq + Hash + Clone> IndexDescriptor for NonUniqueIndex<K> {
    type Key = K;
    type Uniqueness = NonUniqueTag;
}

/// Trait for accessing values through a unique index
pub trait UniqueIndexAccess<K, V, I> {
    fn get_by(&self, key: &K) -> Option<&V>;
    fn get_mut_by(&mut self, key: &K) -> Option<&mut V>;
}

/// Trait for accessing values through a non-unique index
pub trait NonUniqueIndexAccess<K, V, I> {
    fn get_all_by<'a>(&'a self, key: &K) -> impl Iterator<Item = &'a V> + 'a
    where
        V: 'a;
    fn modify_all_by<F>(&mut self, key: &K, f: F)
    where
        F: FnMut(&mut V);
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
pub struct MultiIndexMap<K1, V, I2, I3, I4>
where
    K1: Eq + Hash,
    I2: IndexDescriptor,
    I3: IndexDescriptor,
    I4: IndexDescriptor,
{
    primary: HashMap<Arc<K1>, V>,

    // Secondary indices
    secondary_unique: HashMap<I2::Key, Arc<K1>>,
    secondary_multi: HashMap<I2::Key, Vec<Arc<K1>>>,

    // Tertiary indices
    tertiary_unique: HashMap<I3::Key, Arc<K1>>,
    tertiary_multi: HashMap<I3::Key, Vec<Arc<K1>>>,

    // Quaternary indices
    quaternary_unique: HashMap<I4::Key, Arc<K1>>,
    quaternary_multi: HashMap<I4::Key, Vec<Arc<K1>>>,
    _marker: PhantomData<(I2, I3, I4)>,
}

impl<K1, V, I2, I3, I4> Default for MultiIndexMap<K1, V, I2, I3, I4>
where
    K1: Eq + Hash + Clone,
    I2: IndexDescriptor,
    I3: IndexDescriptor,
    I4: IndexDescriptor,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K1, V, I2, I3, I4> MultiIndexMap<K1, V, I2, I3, I4>
where
    K1: Eq + Hash + Clone,
    I2: IndexDescriptor,
    I3: IndexDescriptor,
    I4: IndexDescriptor,
{
    /// Creates a new empty MultiIndexMap.
    pub fn new() -> Self {
        Self {
            primary: HashMap::new(),
            secondary_unique: HashMap::new(),
            secondary_multi: HashMap::new(),
            tertiary_unique: HashMap::new(),
            tertiary_multi: HashMap::new(),
            quaternary_unique: HashMap::new(),
            quaternary_multi: HashMap::new(),
            _marker: PhantomData,
        }
    }
    /// Returns the number of entries in the primary map.
    pub fn length(&self) -> usize {
        self.primary.len()
    }

    /// Returns true if the map is empty.
    pub fn is_empty(&self) -> bool {
        self.primary.is_empty()
    }
    /// Inserts a new value and associated keys into the map.
    /// Inserts the primary key and value first, then updates the secondary, tertiary,
    /// and quaternary indices based on their uniqueness.
    pub fn insert(&mut self, k1: K1, k2: I2::Key, k3: I3::Key, k4: I4::Key, v: V) {
        let primary_key = Arc::new(k1);

        // Insert into primary storage
        self.primary.insert(primary_key.clone(), v);

        // Handle secondary index based on uniqueness
        if std::any::TypeId::of::<I2::Uniqueness>() == std::any::TypeId::of::<UniqueTag>() {
            self.secondary_unique.insert(k2, primary_key.clone());
        } else {
            self.secondary_multi
                .entry(k2)
                .or_insert(Vec::new())
                .push(primary_key.clone());
        }

        // Handle tertiary index based on uniqueness
        if std::any::TypeId::of::<I3::Uniqueness>() == std::any::TypeId::of::<UniqueTag>() {
            self.tertiary_unique.insert(k3.clone(), primary_key.clone());
        } else {
            self.tertiary_multi
                .entry(k3.clone())
                .and_modify(|vec| vec.push(primary_key.clone()))
                .or_insert_with(|| vec![primary_key.clone()]);
        }

        // Handle quaternary index based on uniqueness
        if std::any::TypeId::of::<I4::Uniqueness>() == std::any::TypeId::of::<UniqueTag>() {
            self.quaternary_unique
                .insert(k4.clone(), primary_key.clone());
        } else {
            self.quaternary_multi
                .entry(k4.clone())
                .and_modify(|vec| vec.push(primary_key.clone()))
                .or_insert_with(|| vec![primary_key.clone()]);
        }
    }

    /// Removes a value and all its indexes using the primary key.
    pub fn remove(&mut self, k1: &K1) -> Option<V> {
        // Remove from primary storage
        let removed = self.primary.remove(k1)?;

        // Remove from secondary index
        if std::any::TypeId::of::<I2::Uniqueness>() == std::any::TypeId::of::<UniqueTag>() {
            // For unique indexes, just remove the entry that points to this k1
            self.secondary_unique.retain(|_, v| v.as_ref() != k1);
        } else {
            // For non-unique indexes, remove k1 from any vectors it appears in
            self.secondary_multi.retain(|_, vec| {
                vec.retain(|x| x.as_ref() != k1);
                !vec.is_empty()
            });
        }

        // Remove from tertiary index
        if std::any::TypeId::of::<I3::Uniqueness>() == std::any::TypeId::of::<UniqueTag>() {
            // For unique indexes, just remove the entry that points to this k1
            self.tertiary_unique.retain(|_, v| v.as_ref() != k1);
        } else {
            // For non-unique indexes, remove k1 from any vectors it appears in
            self.tertiary_multi.retain(|_, vec| {
                vec.retain(|x| x.as_ref() != k1);
                !vec.is_empty()
            });
        }

        // Remove from quaternary index
        if std::any::TypeId::of::<I4::Uniqueness>() == std::any::TypeId::of::<UniqueTag>() {
            self.quaternary_unique.retain(|_, v| v.as_ref() != k1);
        } else {
            self.quaternary_multi.retain(|_, vec| {
                vec.retain(|x| x.as_ref() != k1);
                !vec.is_empty()
            });
        }

        Some(removed)
    }

    /// Updates an existing value using the primary key.
    /// Only updates if the primary key exists; indexes remain unchanged.
    pub fn update(&mut self, k1: &K1, new_value: V) -> Option<V> {
        if !self.primary.contains_key(k1) {
            return None;
        }

        let arc_key = self
            .primary
            .keys()
            .find(|key| key.as_ref() == k1)
            .cloned()?;

        // Only update the value in primary storage
        self.primary.insert(arc_key, new_value)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.primary.values()
    }
}

// Implement unique access for primary key.
impl<K1, V, I2, I3, I4> UniqueIndexAccess<K1, V, Primary> for MultiIndexMap<K1, V, I2, I3, I4>
where
    K1: Eq + Hash + Clone,
    I2: IndexDescriptor,
    I3: IndexDescriptor,
    I4: IndexDescriptor,
{
    fn get_by(&self, key: &K1) -> Option<&V> {
        self.primary.get(key)
    }

    fn get_mut_by(&mut self, key: &K1) -> Option<&mut V> {
        self.primary.get_mut(key)
    }
}

// Implement unique access for secondary key.
impl<K1, V, K2, I3, I4> UniqueIndexAccess<K2, V, Secondary>
    for MultiIndexMap<K1, V, UniqueIndex<K2>, I3, I4>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone + 'static,
    I3: IndexDescriptor,
    I4: IndexDescriptor,
{
    fn get_by(&self, key: &K2) -> Option<&V> {
        let primary_key = self.secondary_unique.get(key)?;
        self.primary.get(primary_key)
    }

    fn get_mut_by(&mut self, key: &K2) -> Option<&mut V> {
        let primary_key = self.secondary_unique.get(key)?.clone();
        self.primary.get_mut(&primary_key)
    }
}

// Implement non-unique access for secondary key.
impl<K1, V, K2, I3, I4> NonUniqueIndexAccess<K2, V, Secondary>
    for MultiIndexMap<K1, V, NonUniqueIndex<K2>, I3, I4>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone + 'static,
    I3: IndexDescriptor,
    I4: IndexDescriptor,
{
    fn get_all_by<'a>(&'a self, key: &K2) -> impl Iterator<Item = &'a V> + 'a
    where
        V: 'a,
    {
        self.secondary_multi
            .get(key)
            .into_iter()
            .flatten()
            .flat_map(|key| self.primary.get(key))
    }

    fn modify_all_by<F>(&mut self, key: &K2, mut f: F)
    where
        F: FnMut(&mut V),
    {
        if let Some(keys) = self.secondary_multi.get(key) {
            let keys = keys.clone();
            for primary_key in keys {
                if let Some(value) = self.primary.get_mut(&primary_key) {
                    f(value);
                }
            }
        }
    }
}

// Implement unique access for tertiary key.
impl<K1, V, I2, K3, I4> UniqueIndexAccess<K3, V, Tertiary>
    for MultiIndexMap<K1, V, I2, UniqueIndex<K3>, I4>
where
    K1: Eq + Hash + Clone,
    I2: IndexDescriptor,
    K3: Eq + Hash + Clone + 'static,
    I4: IndexDescriptor,
{
    fn get_by(&self, key: &K3) -> Option<&V> {
        let primary_key = self.tertiary_unique.get(key)?;
        self.primary.get(primary_key)
    }

    fn get_mut_by(&mut self, key: &K3) -> Option<&mut V> {
        let primary_key = self.tertiary_unique.get(key)?.clone();
        self.primary.get_mut(&primary_key)
    }
}

// Implement non-unique access for tertiary key.
impl<K1, V, I2, K3, I4> NonUniqueIndexAccess<K3, V, Tertiary>
    for MultiIndexMap<K1, V, I2, NonUniqueIndex<K3>, I4>
where
    K1: Eq + Hash + Clone,
    I2: IndexDescriptor,
    K3: Eq + Hash + Clone + 'static,
    I4: IndexDescriptor,
{
    fn get_all_by<'a>(&'a self, key: &K3) -> impl Iterator<Item = &'a V> + 'a
    where
        V: 'a,
    {
        self.tertiary_multi
            .get(key)
            .into_iter()
            .flat_map(|keys| keys.iter().filter_map(|k1| self.primary.get(k1)))
    }

    fn modify_all_by<F>(&mut self, key: &K3, mut f: F)
    where
        F: FnMut(&mut V),
    {
        if let Some(keys) = self.tertiary_multi.get(key) {
            let keys = keys.clone();
            for primary_key in keys {
                if let Some(value) = self.primary.get_mut(&primary_key) {
                    f(value);
                }
            }
        }
    }
}

// Implement unique access for quaternary key.
impl<K1, V, I2, I3, K4> UniqueIndexAccess<K4, V, Quaternary>
    for MultiIndexMap<K1, V, I2, I3, UniqueIndex<K4>>
where
    K1: Eq + Hash + Clone,
    I2: IndexDescriptor,
    I3: IndexDescriptor,
    K4: Eq + Hash + Clone + 'static,
{
    fn get_by(&self, key: &K4) -> Option<&V> {
        let primary_key = self.quaternary_unique.get(key)?;
        self.primary.get(primary_key)
    }

    fn get_mut_by(&mut self, key: &K4) -> Option<&mut V> {
        let primary_key = self.quaternary_unique.get(key)?.clone();
        self.primary.get_mut(&primary_key)
    }
}

// Implement non-unique access for quaternary key.
impl<K1, V, I2, I3, K4> NonUniqueIndexAccess<K4, V, Quaternary>
    for MultiIndexMap<K1, V, I2, I3, NonUniqueIndex<K4>>
where
    K1: Eq + Hash + Clone,
    I2: IndexDescriptor,
    I3: IndexDescriptor,
    K4: Eq + Hash + Clone + 'static,
{
    fn get_all_by<'a>(&'a self, key: &K4) -> impl Iterator<Item = &'a V> + 'a
    where
        V: 'a,
    {
        self.quaternary_multi
            .get(key)
            .into_iter()
            .flat_map(|keys| keys.iter().filter_map(|k1| self.primary.get(k1)))
    }

    fn modify_all_by<F>(&mut self, key: &K4, mut f: F)
    where
        F: FnMut(&mut V),
    {
        if let Some(keys) = self.quaternary_multi.get(key) {
            let keys = keys.clone();
            for primary_key in keys {
                if let Some(value) = self.primary.get_mut(&primary_key) {
                    f(value);
                }
            }
        }
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
            TestValue,
            UniqueIndex<String>,
            UniqueIndex<bool>,
            UniqueIndex<char>,
        > = MultiIndexMap::new();

        let value = TestValue {
            id: 1,
            data: "test".to_string(),
        };

        // Test insertion with quaternary key 'a'
        map.insert(1, "key1".to_string(), true, 'a', value.clone());

        // Test primary key access
        assert_eq!(map.get_by(&1), Some(&value));

        // Test secondary key access
        assert_eq!(map.get_by(&"key1".to_string()), Some(&value));

        // Test tertiary key access
        assert_eq!(map.get_by(&true), Some(&value));

        // Test quaternary key access
        assert_eq!(map.get_by(&'a'), Some(&value));

        // Test update
        let new_value = TestValue {
            id: 1,
            data: "updated".to_string(),
        };
        map.update(&1, new_value.clone());
        assert_eq!(map.get_by(&1), Some(&new_value));

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
            TestValue,
            NonUniqueIndex<String>,
            NonUniqueIndex<bool>,
            NonUniqueIndex<char>,
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
        map.insert(1, "shared_key".to_string(), true, 'z', value1.clone());
        map.insert(2, "shared_key".to_string(), true, 'z', value2.clone());

        // Test primary key access (still unique)
        assert_eq!(map.get_by(&1), Some(&value1));
        assert_eq!(map.get_by(&2), Some(&value2));

        // Test secondary key access (non-unique)
        let secondary_values: Vec<_> = map.get_all_by(&"shared_key".to_string()).collect();
        assert_eq!(secondary_values.len(), 2);
        assert!(secondary_values.contains(&&value1));
        assert!(secondary_values.contains(&&value2));

        // Test tertiary key access (non-unique)
        let tertiary_values: Vec<_> = map.get_all_by(&true).collect();
        assert_eq!(tertiary_values.len(), 2);
        assert!(tertiary_values.contains(&&value1));
        assert!(tertiary_values.contains(&&value2));

        // Test quaternary key access (non-unique)
        let quaternary_values: Vec<_> = map.get_all_by(&'z').collect();
        assert_eq!(quaternary_values.len(), 2);
        assert!(quaternary_values.contains(&&value1));
        assert!(quaternary_values.contains(&&value2));

        // Test removal maintains other entries
        map.remove(&1);
        assert_eq!(map.get_by(&1), None);
        assert_eq!(map.get_by(&2), Some(&value2));

        let remaining_secondary: Vec<_> = map.get_all_by(&"shared_key".to_string()).collect();
        assert_eq!(remaining_secondary.len(), 1);
        assert_eq!(remaining_secondary[0], &value2);
    }

    #[test]
    fn test_mixed_uniqueness() {
        // Mixed: unique secondary, non-unique tertiary, unique quaternary.
        let mut map: MultiIndexMap<
            i32,
            TestValue,
            UniqueIndex<String>,
            NonUniqueIndex<bool>,
            UniqueIndex<char>,
        > = MultiIndexMap::new();

        let value1 = TestValue {
            id: 1,
            data: "test1".to_string(),
        };
        let value2 = TestValue {
            id: 2,
            data: "test2".to_string(),
        };

        // Insert values with unique secondary keys but shared tertiary and different quaternary
        // keys.
        map.insert(1, "key1".to_string(), true, 'q', value1.clone());
        map.insert(2, "key2".to_string(), true, 'r', value2.clone());

        // Test unique secondary key access
        assert_eq!(map.get_by(&"key1".to_string()), Some(&value1));
        assert_eq!(map.get_by(&"key2".to_string()), Some(&value2));

        // Test non-unique tertiary key access
        let tertiary_values: Vec<_> = map.get_all_by(&true).collect();
        assert_eq!(tertiary_values.len(), 2);
        assert!(tertiary_values.contains(&&value1));
        assert!(tertiary_values.contains(&&value2));

        // Test unique quaternary key access
        assert_eq!(map.get_by(&'q'), Some(&value1));
        assert_eq!(map.get_by(&'r'), Some(&value2));
    }

    #[test]
    fn test_empty_cases() {
        let mut map: MultiIndexMap<
            i32,
            TestValue,
            UniqueIndex<String>,
            UniqueIndex<bool>,
            UniqueIndex<char>,
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

    #[test]
    fn test_get_mut_by() {
        // Using unique indices for all secondary, tertiary, and quaternary keys.
        let mut map: MultiIndexMap<
            i32,
            TestValue,
            UniqueIndex<String>,
            UniqueIndex<bool>,
            UniqueIndex<char>,
        > = MultiIndexMap::new();

        let value = TestValue {
            id: 1,
            data: "original".to_string(),
        };

        // Test insertion
        map.insert(1, "key1".to_string(), true, 'a', value.clone());

        // Test mutable access via primary key
        if let Some(mut_ref) = map.get_mut_by(&1) {
            mut_ref.data = "modified_primary".to_string();
        }
        assert_eq!(map.get_by(&1).unwrap().data, "modified_primary");

        // Test mutable access via secondary key
        if let Some(mut_ref) = map.get_mut_by(&"key1".to_string()) {
            mut_ref.data = "modified_secondary".to_string();
        }
        assert_eq!(map.get_by(&1).unwrap().data, "modified_secondary");

        // Test mutable access via tertiary key
        if let Some(mut_ref) = map.get_mut_by(&true) {
            mut_ref.data = "modified_tertiary".to_string();
        }
        assert_eq!(map.get_by(&1).unwrap().data, "modified_tertiary");

        // Test mutable access via quaternary key
        if let Some(mut_ref) = map.get_mut_by(&'a') {
            mut_ref.data = "modified_quaternary".to_string();
        }
        assert_eq!(map.get_by(&1).unwrap().data, "modified_quaternary");

        // Test that all index access methods see the same modified value
        assert_eq!(
            map.get_by(&"key1".to_string()).unwrap().data,
            "modified_quaternary"
        );
        assert_eq!(map.get_by(&true).unwrap().data, "modified_quaternary");
        assert_eq!(map.get_by(&'a').unwrap().data, "modified_quaternary");

        // Test access to non-existent keys returns None
        assert!(map.get_mut_by(&2).is_none());
        assert!(map.get_mut_by(&"nonexistent".to_string()).is_none());
        assert!(map.get_mut_by(&false).is_none());
        assert!(map.get_mut_by(&'z').is_none());
    }

    #[test]
    fn test_modify_all_by() {
        // Using non-unique indices for all secondary, tertiary, and quaternary keys.
        let mut map: MultiIndexMap<
            i32,
            TestValue,
            NonUniqueIndex<String>,
            NonUniqueIndex<bool>,
            NonUniqueIndex<char>,
        > = MultiIndexMap::new();

        let value1 = TestValue {
            id: 1,
            data: "original1".to_string(),
        };
        let value2 = TestValue {
            id: 2,
            data: "original2".to_string(),
        };
        let value3 = TestValue {
            id: 3,
            data: "original3".to_string(),
        };

        // Insert values with shared keys
        map.insert(1, "shared_key".to_string(), true, 'z', value1.clone());
        map.insert(2, "shared_key".to_string(), true, 'z', value2.clone());
        map.insert(3, "other_key".to_string(), false, 'y', value3.clone());

        // Test mutable access via secondary key
        let mut counter = 0;
        map.modify_all_by(&"shared_key".to_string(), |value| {
            counter += 1;
            value.data = format!("modified_secondary_{counter}");
        });

        // Verify both values were modified
        assert!(
            map.get_by(&1)
                .unwrap()
                .data
                .starts_with("modified_secondary_")
        );
        assert!(
            map.get_by(&2)
                .unwrap()
                .data
                .starts_with("modified_secondary_")
        );
        assert_eq!(map.get_by(&3).unwrap().data, "original3"); // Unchanged

        // Test mutable access via tertiary key
        map.modify_all_by(&true, |value| {
            value.data = format!("modified_tertiary_{}", value.id);
        });

        // Verify both values were modified
        assert_eq!(map.get_by(&1).unwrap().data, "modified_tertiary_1");
        assert_eq!(map.get_by(&2).unwrap().data, "modified_tertiary_2");
        assert_eq!(map.get_by(&3).unwrap().data, "original3"); // Unchanged

        // Test mutable access via quaternary key
        map.modify_all_by(&'z', |value| {
            value.data = format!("modified_quaternary_{}", value.id);
        });

        // Verify both values were modified
        assert_eq!(map.get_by(&1).unwrap().data, "modified_quaternary_1");
        assert_eq!(map.get_by(&2).unwrap().data, "modified_quaternary_2");
        assert_eq!(map.get_by(&3).unwrap().data, "original3"); // Unchanged

        // Test access to non-existent keys does nothing
        map.modify_all_by(&"nonexistent".to_string(), |_value| {
            panic!("Should not be called for non-existent key");
        });

        map.modify_all_by(&'x', |_value| {
            panic!("Should not be called for non-existent key");
        });
    }
}
