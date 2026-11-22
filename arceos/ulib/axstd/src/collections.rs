//! Allocating collections, including a hash table for kernel applications.

pub use alloc::collections::*;

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher};

/// FNV-1a hasher for the default kernel hash table.
pub struct DefaultHasher(u64);

impl Default for DefaultHasher {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}

impl Hasher for DefaultHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

/// A hash table with separate chaining and automatic growth.
///
/// Its default hasher is deterministic; applications accepting untrusted keys
/// can supply a keyed hasher with `with_hasher`.
pub struct HashMap<K, V, S = BuildHasherDefault<DefaultHasher>> {
    buckets: Vec<Vec<(K, V)>>,
    len: usize,
    hash_builder: S,
}

impl<K, V> HashMap<K, V> {
    pub fn new() -> Self {
        Self::with_hasher(BuildHasherDefault::default())
    }

    pub fn with_capacity(capacity: usize) -> Self
    where
        K: Eq + Hash,
    {
        let mut map = Self::new();
        map.reserve(capacity);
        map
    }
}

impl<K, V, S> HashMap<K, V, S> {
    pub const fn with_hasher(hash_builder: S) -> Self {
        Self {
            buckets: Vec::new(),
            len: 0,
            hash_builder,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.buckets.len() / 4 * 3
    }

    pub fn clear(&mut self) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        self.len = 0;
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.buckets.iter().flatten().map(|(k, v)| (k, v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.buckets.iter_mut().flatten().map(|(k, v)| (&*k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }
}

impl<K: Eq + Hash, V, S: BuildHasher> HashMap<K, V, S> {
    fn bucket<Q: Hash + ?Sized>(&self, key: &Q) -> usize {
        self.hash_builder.hash_one(key) as usize & (self.buckets.len() - 1)
    }

    pub fn reserve(&mut self, additional: usize) {
        let required = self
            .len
            .checked_add(additional)
            .expect("hash map capacity overflow");
        if required <= self.capacity() {
            return;
        }
        let count = required
            .checked_mul(2)
            .and_then(usize::checked_next_power_of_two)
            .expect("hash map capacity overflow")
            .max(8);
        let mut buckets = Vec::with_capacity(count);
        buckets.resize_with(count, Vec::new);
        let old = core::mem::replace(&mut self.buckets, buckets);
        for (key, value) in old.into_iter().flatten() {
            let index = self.bucket(&key);
            self.buckets[index].push((key, value));
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some(old) = self.get_mut(&key) {
            return Some(core::mem::replace(old, value));
        }
        self.reserve(1);
        let index = self.bucket(&key);
        self.buckets[index].push((key, value));
        self.len += 1;
        None
    }

    pub fn get<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        if self.buckets.is_empty() {
            return None;
        }
        self.buckets[self.bucket(key)]
            .iter()
            .find(|(k, _)| k.borrow() == key)
            .map(|(_, v)| v)
    }

    pub fn get_mut<Q: Eq + Hash + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        if self.buckets.is_empty() {
            return None;
        }
        let index = self.bucket(key);
        self.buckets[index]
            .iter_mut()
            .find(|(k, _)| k.borrow() == key)
            .map(|(_, v)| v)
    }

    pub fn contains_key<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.get(key).is_some()
    }

    pub fn remove<Q: Eq + Hash + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        if self.buckets.is_empty() {
            return None;
        }
        let index = self.bucket(key);
        let bucket = &mut self.buckets[index];
        let pos = bucket.iter().position(|(k, _)| k.borrow() == key)?;
        self.len -= 1;
        Some(bucket.swap_remove(pos).1)
    }
}

impl<K, V, S: Default> Default for HashMap<K, V, S> {
    fn default() -> Self {
        Self::with_hasher(S::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, string::String};

    #[test]
    fn grows_and_supports_borrowed_keys() {
        let mut map = HashMap::new();
        for value in 0..50_000 {
            assert_eq!(map.insert(format!("key_{value}"), value), None);
        }
        assert_eq!(map.len(), 50_000);
        for value in 0..50_000 {
            assert_eq!(map.get(format!("key_{value}").as_str()), Some(&value));
        }
        assert_eq!(map.insert(String::from("key_1"), 42), Some(1));
        assert_eq!(map.remove("key_1"), Some(42));
        assert!(!map.contains_key("key_1"));
        assert_eq!(map.iter().count(), 49_999);
        map.clear();
        assert!(map.is_empty());
    }

    #[derive(Default)]
    struct CollisionHasher;

    impl Hasher for CollisionHasher {
        fn finish(&self) -> u64 {
            0
        }
        fn write(&mut self, _: &[u8]) {}
    }

    #[test]
    fn collisions_preserve_distinct_keys() {
        let mut map = HashMap::with_hasher(BuildHasherDefault::<CollisionHasher>::default());
        for i in 0..100 {
            map.insert(i, i * 2);
        }
        for i in 0..100 {
            assert_eq!(map.remove(&i), Some(i * 2));
        }
        assert!(map.is_empty());
    }
}
