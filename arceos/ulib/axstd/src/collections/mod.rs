//! Collection types available to ArceOS applications.
//!
//! This mirrors `std::collections`: the ordered collections come from
//! [`alloc::collections`], while [`HashMap`] is provided here because `alloc`
//! itself has no hash map implementation.

pub use alloc::collections::{BTreeMap, BTreeSet, BinaryHeap, LinkedList, VecDeque};

mod hash_map;
pub use self::hash_map::HashMap;
