//! A small open-addressing hash map for `no_std` environments.

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::hash::{Hash, Hasher};
use core::iter::FromIterator;
use core::ops::Index;

enum Slot<K, V> {
    Empty,
    Deleted,
    Full(K, V),
}

/// A hash map implemented with linear probing.
///
/// The default hash function is stable across boots, which keeps the map
/// usable before a random number generator is available.
pub struct HashMap<K, V> {
    slots: Vec<Slot<K, V>>,
    len: usize,
}

impl<K, V> HashMap<K, V> {
    /// Creates an empty map.
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            len: 0,
        }
    }

    /// Creates an empty map with room for at least `capacity` entries.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: empty_slots(table_size(capacity)),
            len: 0,
        }
    }

    /// Returns the number of entries in the map.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the map contains no entries.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Removes all entries.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.len = 0;
    }

    /// Returns a reference to the value corresponding to `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let idx = self.find(key)?;
        match &self.slots[idx] {
            Slot::Full(_, value) => Some(value),
            _ => None,
        }
    }

    /// Returns a mutable reference to the value corresponding to `key`.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let idx = self.find(key)?;
        match &mut self.slots[idx] {
            Slot::Full(_, value) => Some(value),
            _ => None,
        }
    }

    /// Returns `true` if the map contains `key`.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.find(key).is_some()
    }

    /// Inserts a key-value pair, returning the previous value if it existed.
    pub fn insert(&mut self, key: K, value: V) -> Option<V>
    where
        K: Hash + Eq,
    {
        self.reserve_one();

        match self.probe_for_insert(&key) {
            Ok(idx) => match &mut self.slots[idx] {
                Slot::Full(_, old) => Some(core::mem::replace(old, value)),
                _ => unreachable!(),
            },
            Err(idx) => {
                self.slots[idx] = Slot::Full(key, value);
                self.len += 1;
                None
            }
        }
    }

    /// Removes a key from the map, returning its value if it existed.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let idx = self.find(key)?;
        let old = core::mem::replace(&mut self.slots[idx], Slot::Deleted);
        self.len -= 1;
        match old {
            Slot::Full(_, value) => Some(value),
            _ => None,
        }
    }

    /// Iterates over the entries in an unspecified order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            inner: self.slots.iter(),
        }
    }

    /// Iterates over the entries with mutable access to their values.
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        IterMut {
            inner: self.slots.iter_mut(),
        }
    }

    /// Iterates over the keys.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(key, _)| key)
    }

    /// Iterates over the values.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, value)| value)
    }

    fn find<Q>(&self, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut idx = make_hash(key) & mask;
        loop {
            match &self.slots[idx] {
                Slot::Empty => return None,
                Slot::Full(stored, _) if stored.borrow() == key => return Some(idx),
                _ => idx = (idx + 1) & mask,
            }
        }
    }

    fn probe_for_insert<Q>(&self, key: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let mask = self.slots.len() - 1;
        let mut idx = make_hash(key) & mask;
        let mut first_deleted = None;
        loop {
            match &self.slots[idx] {
                Slot::Full(stored, _) if stored.borrow() == key => return Ok(idx),
                Slot::Empty => return Err(first_deleted.unwrap_or(idx)),
                Slot::Deleted => {
                    first_deleted.get_or_insert(idx);
                }
                Slot::Full(..) => {}
            }
            idx = (idx + 1) & mask;
        }
    }

    fn reserve_one(&mut self)
    where
        K: Hash + Eq,
    {
        // Keep at least one quarter of the table empty so probes terminate
        // quickly and an insertion always finds an `Empty` slot.
        if self.slots.is_empty() || (self.len + 1) * 4 >= self.slots.len() * 3 {
            self.resize(table_size(self.len + 1));
        }
    }

    fn resize(&mut self, new_size: usize)
    where
        K: Hash + Eq,
    {
        let old = core::mem::replace(&mut self.slots, empty_slots(new_size));
        self.len = 0;
        for slot in old {
            if let Slot::Full(key, value) = slot {
                match self.probe_for_insert(&key) {
                    Err(idx) => {
                        self.slots[idx] = Slot::Full(key, value);
                        self.len += 1;
                    }
                    Ok(_) => unreachable!("duplicate key while resizing"),
                }
            }
        }
    }
}

impl<K, V> Default for HashMap<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, Q> Index<&Q> for HashMap<K, V>
where
    K: Borrow<Q>,
    Q: Hash + Eq + ?Sized,
{
    type Output = V;

    fn index(&self, key: &Q) -> &V {
        self.get(key).expect("no entry found for key")
    }
}

/// Borrowing iterator over a [`HashMap`].
pub struct Iter<'a, K, V> {
    inner: core::slice::Iter<'a, Slot<K, V>>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        for slot in self.inner.by_ref() {
            if let Slot::Full(key, value) = slot {
                return Some((key, value));
            }
        }
        None
    }
}

/// Mutably borrowing iterator over a [`HashMap`].
pub struct IterMut<'a, K, V> {
    inner: core::slice::IterMut<'a, Slot<K, V>>,
}

impl<'a, K, V> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    fn next(&mut self) -> Option<Self::Item> {
        for slot in self.inner.by_ref() {
            if let Slot::Full(key, value) = slot {
                return Some((key, value));
            }
        }
        None
    }
}

impl<'a, K, V> IntoIterator for &'a HashMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K, V> Extend<(K, V)> for HashMap<K, V>
where
    K: Hash + Eq,
{
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        for (key, value) in iter {
            self.insert(key, value);
        }
    }
}

impl<K, V> FromIterator<(K, V)> for HashMap<K, V>
where
    K: Hash + Eq,
{
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = Self::new();
        map.extend(iter);
        map
    }
}

fn table_size(capacity: usize) -> usize {
    let wanted = capacity.saturating_mul(4).div_ceil(3).max(8);
    wanted.next_power_of_two()
}

fn empty_slots<K, V>(size: usize) -> Vec<Slot<K, V>> {
    let mut slots = Vec::with_capacity(size);
    slots.resize_with(size, || Slot::Empty);
    slots
}

struct FnvHasher(u64);

impl Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
}

fn make_hash<Q: Hash + ?Sized>(key: &Q) -> usize {
    let mut hasher = FnvHasher(0xcbf2_9ce4_8422_2325);
    key.hash(&mut hasher);
    hasher.finish() as usize
}
