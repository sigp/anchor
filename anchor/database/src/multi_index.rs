use std::any::TypeId;
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;

/// Marker trait for uniquely identifying indices.
pub trait Unique {}

/// Marker trait for non-uniquely identifying indices.
pub trait NotUnique {}

/// Marker types for unique and non-unique tags.
#[derive(Debug)]
pub enum UniqueTag {}
impl Unique for UniqueTag {}

#[derive(Debug)]
pub enum NonUniqueTag {}
impl NotUnique for NonUniqueTag {}

/// Marker types for index access.
pub enum Primary {}
pub enum Secondary {}
pub enum Tertiary {}
pub enum Quaternary {}

/// Trait for accessing a value through a unique index.
pub trait UniqueIndex<K, V, I> {
    fn get_by(&self, key: &K) -> Option<V>;
}

/// Trait for accessing values through a non-unique index.
pub trait NonUniqueIndex<K, V, I> {
    fn get_all_by(&self, key: &K) -> Option<Vec<V>>;
}

/// A multi-index map with one required primary key and up to three optional secondary indices.
///
/// - **K1:** Primary key type (always required).
/// - **V:** Stored value type.
/// - **K2:** Secondary key type (optional, defaults to `()`).
/// - **U2:** Tag for secondary uniqueness (either `UniqueTag` or `NonUniqueTag`; defaults to `NonUniqueTag`).
/// - **K3:** Tertiary key type (optional, defaults to `()`).
/// - **U3:** Tag for tertiary uniqueness (defaults to `NonUniqueTag`).
/// - **K4:** Quaternary key type (optional, defaults to `()`).
/// - **U4:** Tag for quaternary uniqueness (defaults to `NonUniqueTag`).
///
/// When the user does not need an additional index, they can simply use the default value for that type,
/// and the underlying implementation will ignore it.
#[derive(Debug)]
pub struct MultiIndexMap<
    K1,
    V,
    K2 = (),
    U2 = NonUniqueTag,
    K3 = (),
    U3 = NonUniqueTag,
    K4 = (),
    U4 = NonUniqueTag,
> where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    V: Clone,
    U2: 'static,
    U3: 'static,
    U4: 'static,
{
    primary: HashMap<K1, V>,
    // Optional secondary index.
    secondary_unique: Option<HashMap<K2, K1>>,
    secondary_multi: Option<HashMap<K2, Vec<K1>>>,
    // Optional tertiary index.
    tertiary_unique: Option<HashMap<K3, K1>>,
    tertiary_multi: Option<HashMap<K3, Vec<K1>>>,
    // Optional quaternary index.
    quaternary_unique: Option<HashMap<K4, K1>>,
    quaternary_multi: Option<HashMap<K4, Vec<K1>>>,
    _marker: PhantomData<(U2, U3, U4)>,
}

impl<K1, V, K2, U2, K3, U3, K4, U4> Default for MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    V: Clone,
    U2: 'static,
    U3: 'static,
    U4: 'static,
{
    fn default() -> Self {
        Self {
            primary: HashMap::new(),
            secondary_unique: if TypeId::of::<K2>() == TypeId::of::<()>() {
                None
            } else {
                Some(HashMap::new())
            },
            secondary_multi: if TypeId::of::<K2>() == TypeId::of::<()>() {
                None
            } else {
                Some(HashMap::new())
            },
            tertiary_unique: if TypeId::of::<K3>() == TypeId::of::<()>() {
                None
            } else {
                Some(HashMap::new())
            },
            tertiary_multi: if TypeId::of::<K3>() == TypeId::of::<()>() {
                None
            } else {
                Some(HashMap::new())
            },
            quaternary_unique: if TypeId::of::<K4>() == TypeId::of::<()>() {
                None
            } else {
                Some(HashMap::new())
            },
            quaternary_multi: if TypeId::of::<K4>() == TypeId::of::<()>() {
                None
            } else {
                Some(HashMap::new())
            },
            _marker: PhantomData,
        }
    }
}

impl<K1, V, K2, U2, K3, U3, K4, U4> MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    V: Clone,
    U2: 'static,
    U3: 'static,
    U4: 'static,
{
    /// Creates a new empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of entries in the primary map.
    pub fn length(&self) -> usize {
        self.primary.len()
    }

    /// Inserts a new value along with its associated keys.
    ///
    /// For indices whose key type is the dummy type `()`, the corresponding key argument is ignored.
    pub fn insert(&mut self, k1: &K1, k2: &K2, k3: &K3, k4: &K4, v: V) {
        self.primary.insert(k1.clone(), v);

        // Secondary index update.
        if let Some(ref mut unique) = self.secondary_unique {
            if TypeId::of::<U2>() == TypeId::of::<UniqueTag>() {
                unique.insert(k2.clone(), k1.clone());
            } else if let Some(ref mut multi) = self.secondary_multi {
                multi
                    .entry(k2.clone())
                    .and_modify(|vec| vec.push(k1.clone()))
                    .or_insert_with(|| vec![k1.clone()]);
            }
        }

        // Tertiary index update.
        if let Some(ref mut unique) = self.tertiary_unique {
            if TypeId::of::<U3>() == TypeId::of::<UniqueTag>() {
                unique.insert(k3.clone(), k1.clone());
            } else if let Some(ref mut multi) = self.tertiary_multi {
                multi
                    .entry(k3.clone())
                    .and_modify(|vec| vec.push(k1.clone()))
                    .or_insert_with(|| vec![k1.clone()]);
            }
        }

        // Quaternary index update.
        if let Some(ref mut unique) = self.quaternary_unique {
            if TypeId::of::<U4>() == TypeId::of::<UniqueTag>() {
                unique.insert(k4.clone(), k1.clone());
            } else if let Some(ref mut multi) = self.quaternary_multi {
                multi
                    .entry(k4.clone())
                    .and_modify(|vec| vec.push(k1.clone()))
                    .or_insert_with(|| vec![k1.clone()]);
            }
        }
    }

    /// Removes a value and its indices by the primary key.
    pub fn remove(&mut self, k1: &K1) -> Option<V> {
        let removed = self.primary.remove(k1)?;

        if let Some(ref mut unique) = self.secondary_unique {
            unique.retain(|_, pk| pk != k1);
        }
        if let Some(ref mut multi) = self.secondary_multi {
            multi.retain(|_, vec| {
                vec.retain(|pk| pk != k1);
                !vec.is_empty()
            });
        }

        if let Some(ref mut unique) = self.tertiary_unique {
            unique.retain(|_, pk| pk != k1);
        }
        if let Some(ref mut multi) = self.tertiary_multi {
            multi.retain(|_, vec| {
                vec.retain(|pk| pk != k1);
                !vec.is_empty()
            });
        }

        if let Some(ref mut unique) = self.quaternary_unique {
            unique.retain(|_, pk| pk != k1);
        }
        if let Some(ref mut multi) = self.quaternary_multi {
            multi.retain(|_, vec| {
                vec.retain(|pk| pk != k1);
                !vec.is_empty()
            });
        }

        Some(removed)
    }

    /// Updates the value associated with the given primary key.
    /// The indices remain unchanged.
    pub fn update(&mut self, k1: &K1, new_value: V) -> Option<V> {
        if !self.primary.contains_key(k1) {
            return None;
        }
        self.primary.insert(k1.clone(), new_value)
    }
}

// Implementations for index lookup.

impl<K1, V, K2, U2, K3, U3, K4, U4> UniqueIndex<K1, V, Primary>
    for MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    V: Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
{
    fn get_by(&self, key: &K1) -> Option<V> {
        self.primary.get(key).cloned()
    }
}

impl<K1, V, K2, U2, K3, U3, K4, U4> UniqueIndex<K2, V, Secondary>
    for MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    V: Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    U2: Unique,
{
    fn get_by(&self, key: &K2) -> Option<V> {
        self.secondary_unique
            .as_ref()?
            .get(key)
            .and_then(|pk| self.primary.get(pk).cloned())
    }
}

impl<K1, V, K2, U2, K3, U3, K4, U4> NonUniqueIndex<K2, V, Secondary>
    for MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    V: Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    U2: NotUnique,
{
    fn get_all_by(&self, key: &K2) -> Option<Vec<V>> {
        self.secondary_multi.as_ref()?.get(key).map(|vec| {
            vec.iter()
                .filter_map(|pk| self.primary.get(pk).cloned())
                .collect()
        })
    }
}

impl<K1, V, K2, U2, K3, U3, K4, U4> UniqueIndex<K3, V, Tertiary>
    for MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    V: Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    U3: Unique,
{
    fn get_by(&self, key: &K3) -> Option<V> {
        self.tertiary_unique
            .as_ref()?
            .get(key)
            .and_then(|pk| self.primary.get(pk).cloned())
    }
}

impl<K1, V, K2, U2, K3, U3, K4, U4> NonUniqueIndex<K3, V, Tertiary>
    for MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    V: Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    U3: NotUnique,
{
    fn get_all_by(&self, key: &K3) -> Option<Vec<V>> {
        self.tertiary_multi.as_ref()?.get(key).map(|vec| {
            vec.iter()
                .filter_map(|pk| self.primary.get(pk).cloned())
                .collect()
        })
    }
}

impl<K1, V, K2, U2, K3, U3, K4, U4> UniqueIndex<K4, V, Quaternary>
    for MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    V: Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    U4: Unique,
{
    fn get_by(&self, key: &K4) -> Option<V> {
        self.quaternary_unique
            .as_ref()?
            .get(key)
            .and_then(|pk| self.primary.get(pk).cloned())
    }
}

impl<K1, V, K2, U2, K3, U3, K4, U4> NonUniqueIndex<K4, V, Quaternary>
    for MultiIndexMap<K1, V, K2, U2, K3, U3, K4, U4>
where
    K1: Eq + Hash + Clone,
    V: Clone,
    K2: Eq + Hash + Clone + 'static,
    K3: Eq + Hash + Clone + 'static,
    K4: Eq + Hash + Clone + 'static,
    U4: NotUnique,
{
    fn get_all_by(&self, key: &K4) -> Option<Vec<V>> {
        self.quaternary_multi.as_ref()?.get(key).map(|vec| {
            vec.iter()
                .filter_map(|pk| self.primary.get(pk).cloned())
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct TestValue {
        id: i32,
        data: String,
    }

    #[test]
    fn test_primary_only() {
        // When only the primary index is needed, the optional types default to ().
        let mut map: MultiIndexMap<i32, TestValue> = MultiIndexMap::new();
        let value = TestValue {
            id: 1,
            data: "primary only".to_string(),
        };

        // The caller must still supply dummy keys.
        map.insert(&1, &(), &(), &(), value.clone());
        assert_eq!(
            <MultiIndexMap<_, _> as UniqueIndex<i32, TestValue, Primary>>::get_by(&map, &1),
            Some(value.clone())
        );
        assert_eq!(map.remove(&1), Some(value));
    }

    #[test]
    fn test_with_secondary() {
        // Using a non-dummy secondary key while tertiary and quaternary default to ().
        let mut map: MultiIndexMap<i32, TestValue, String, UniqueTag> = MultiIndexMap::new();
        let value = TestValue {
            id: 1,
            data: "with secondary".to_string(),
        };

        map.insert(&1, &"sec".to_string(), &(), &(), value.clone());
        // Lookup by primary.
        assert_eq!(<MultiIndexMap<_, _, String, UniqueTag> as UniqueIndex<i32, TestValue, Primary>>::get_by(&map, &1), Some(value.clone()));
        // Lookup by secondary.
        assert_eq!(<MultiIndexMap<_, _, String, UniqueTag> as UniqueIndex<String, TestValue, Secondary>>::get_by(&map, &"sec".to_string()), Some(value.clone()));
        assert_eq!(map.remove(&1), Some(value));
    }

    #[test]
    fn test_full_indices() {
        // Use all indices.
        let mut map: MultiIndexMap<
            i32,
            TestValue,
            String,
            UniqueTag,
            bool,
            NonUniqueTag,
            char,
            UniqueTag,
        > = MultiIndexMap::new();
        let value = TestValue {
            id: 1,
            data: "full indices".to_string(),
        };

        map.insert(&1, &"s".to_string(), &true, &'a', value.clone());
        assert_eq!(<MultiIndexMap<_, _, String, UniqueTag, bool, NonUniqueTag, char, UniqueTag> as UniqueIndex<i32, TestValue, Primary>>::get_by(&map, &1), Some(value.clone()));
        assert_eq!(<MultiIndexMap<_, _, String, UniqueTag, bool, NonUniqueTag, char, UniqueTag> as UniqueIndex<String, TestValue, Secondary>>::get_by(&map, &"s".to_string()), Some(value.clone()));
        assert_eq!(<MultiIndexMap<_, _, String, UniqueTag, bool, NonUniqueTag, char, UniqueTag> as NonUniqueIndex<bool, TestValue, Tertiary>>::get_all_by(&map, &true).unwrap().len(), 1);
        assert_eq!(<MultiIndexMap<_, _, String, UniqueTag, bool, NonUniqueTag, char, UniqueTag> as UniqueIndex<char, TestValue, Quaternary>>::get_by(&map, &'a'), Some(value.clone()));
        assert_eq!(map.remove(&1), Some(value));
    }
}
