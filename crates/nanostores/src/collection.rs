use crate::Subscription;
use crate::store::{Listener, StoreInner, StoreLike};
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::sync::Arc;

/// A keyed collection with an explicit order.
///
/// `MapStore` notifies per FIELD (its keys are the struct's field names, fixed
/// at compile time). A list of rows needs the opposite: keys that appear and
/// vanish at runtime — one per item. Without that, touching a single item
/// re-notifies every listener of the whole list, and the UI re-renders rows
/// that did not change.
///
/// Order is kept beside the items, not derived from them: a `HashMap` has no
/// order, and re-sorting on every read would push sorting policy into every
/// consumer. Changing the order notifies under [`ORDER_KEY`], so a listener
/// that only draws one row is not woken by a re-sort.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection<K, V>
where
    K: Eq + Hash + Clone,
{
    items: HashMap<K, V>,
    order: Vec<K>,
}

/// Key under which order changes are announced (items themselves are announced
/// under their own key, so a row listener stays asleep while the list re-sorts).
pub const ORDER_KEY: &str = "@order";

impl<K, V> Default for Collection<K, V>
where
    K: Eq + Hash + Clone,
{
    fn default() -> Self {
        Self {
            items: HashMap::new(),
            order: Vec::new(),
        }
    }
}

impl<K, V> Collection<K, V>
where
    K: Eq + Hash + Clone,
{
    pub fn new(items: HashMap<K, V>, order: Vec<K>) -> Self {
        Self { items, order }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.items.get(key)
    }

    pub fn order(&self) -> &[K] {
        &self.order
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Items in order. Keys present in `order` but missing from `items` are
    /// skipped: the two are set separately, and a caller mid-update should not
    /// see a panic.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.order
            .iter()
            .filter_map(|key| self.items.get(key).map(|value| (key, value)))
    }
}

pub fn collection<K, V>(init: Collection<K, V>) -> CollectionStore<K, V>
where
    K: Eq + Hash + Clone + Display + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    CollectionStore {
        inner: StoreInner::new(init),
    }
}

pub struct CollectionStore<K, V>
where
    K: Eq + Hash + Clone + Display + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    pub(crate) inner: Arc<StoreInner<Collection<K, V>>>,
}

impl<K, V> Clone for CollectionStore<K, V>
where
    K: Eq + Hash + Clone + Display + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<K, V> CollectionStore<K, V>
where
    K: Eq + Hash + Clone + Display + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    pub fn get(&self) -> Collection<K, V> {
        self.inner.get()
    }

    pub fn get_item(&self, key: &K) -> Option<V> {
        self.inner.get().get(key).cloned()
    }

    /// Insert or replace one item. Notifies under this item's key only —
    /// listeners of other rows stay asleep. An item equal to the stored one
    /// notifies nobody (the store compares before writing).
    pub fn set_item(&self, key: K, value: V) {
        let mut next = self.inner.get();
        if !next.items.contains_key(&key) {
            next.order.push(key.clone());
        }
        next.items.insert(key.clone(), value);
        self.inner.set_value(next, Some(key.to_string()), true);
    }

    /// Change one item in place. The closure returns `false` to abort (nothing
    /// is written, nobody is notified) — that is how a no-op update avoids
    /// waking the row.
    pub fn update_item<F>(&self, key: K, update: F)
    where
        F: FnOnce(&mut V) -> bool,
    {
        let mut next = self.inner.get();
        let Some(item) = next.items.get_mut(&key) else {
            return;
        };
        if !update(item) {
            return;
        }
        self.inner.set_value(next, Some(key.to_string()), true);
    }

    /// Drop one item. Order shrinks with it; notification carries the item's key.
    pub fn remove_item(&self, key: &K) {
        let mut next = self.inner.get();
        if next.items.remove(key).is_none() {
            return;
        }
        next.order.retain(|k| k != key);
        self.inner.set_value(next, Some(key.to_string()), true);
    }

    /// Replace the order (sorting, filtering). Announced under [`ORDER_KEY`]:
    /// a listener drawing a single row is not woken by a re-sort.
    pub fn set_order(&self, order: Vec<K>) {
        let mut next = self.inner.get();
        next.order = order;
        self.inner.set_value(next, Some(ORDER_KEY.to_owned()), true);
    }

    /// Replace everything (initial load, reset). No key: this is not a change
    /// of one row, and every listener must see it.
    pub fn set(&self, value: Collection<K, V>) {
        self.inner.set_value(value, None, true);
    }

    /// Listen to ONE item: fires on that item's changes and on nothing else
    /// (not even a re-sort — order lives under its own key).
    pub fn listen_key<F>(&self, key: &K, listener: F) -> Subscription
    where
        F: Fn(&Collection<K, V>, Option<&str>) + Send + Sync + 'static,
    {
        let keys: Arc<[String]> = vec![key.to_string()].into();
        self.inner
            .add_listener(Arc::new(listener), false, Some(keys))
    }

    /// Listen to the ORDER only (the row list itself, not row contents).
    pub fn listen_order<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&Collection<K, V>, Option<&str>) + Send + Sync + 'static,
    {
        let keys: Arc<[String]> = vec![ORDER_KEY.to_owned()].into();
        self.inner
            .add_listener(Arc::new(listener), false, Some(keys))
    }

    pub fn listen<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&Collection<K, V>, Option<&str>) + Send + Sync + 'static,
    {
        self.listen_arc(Arc::new(listener))
    }

    pub fn subscribe<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&Collection<K, V>, Option<&str>) + Send + Sync + 'static,
    {
        self.subscribe_arc(Arc::new(listener))
    }
}

impl<K, V> StoreLike for CollectionStore<K, V>
where
    K: Eq + Hash + Clone + Display + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    type Value = Collection<K, V>;

    fn get(&self) -> Self::Value {
        self.inner.get()
    }

    fn listen_arc(&self, listener: Arc<Listener<Self::Value>>) -> Subscription {
        self.inner.add_listener(listener, false, None)
    }

    fn subscribe_arc(&self, listener: Arc<Listener<Self::Value>>) -> Subscription {
        self.inner.add_listener(listener, true, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flush;
    use std::sync::Mutex;

    fn store() -> CollectionStore<u32, String> {
        collection(Collection::default())
    }

    /// A row listener wakes for its own row and sleeps through the others.
    #[test]
    fn item_listener_is_woken_only_by_its_own_item() {
        let store = store();
        store.set_item(1, "one".into());
        store.set_item(2, "two".into());

        let woken = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&woken);
        let _sub = store.listen_key(&1, move |_, _| {
            *counter.lock().unwrap() += 1;
        });

        store.set_item(2, "TWO".into());
        flush();
        assert_eq!(*woken.lock().unwrap(), 0, "чужая строка не будит");

        store.set_item(1, "ONE".into());
        flush();
        assert_eq!(*woken.lock().unwrap(), 1, "своя — будит");
    }

    /// Re-sorting is announced under the order key, so row listeners stay asleep.
    #[test]
    fn reorder_does_not_wake_item_listeners() {
        let store = store();
        store.set_item(1, "one".into());
        store.set_item(2, "two".into());

        let rows = Arc::new(Mutex::new(0));
        let order = Arc::new(Mutex::new(0));
        let rows_counter = Arc::clone(&rows);
        let order_counter = Arc::clone(&order);
        let _row = store.listen_key(&1, move |_, _| {
            *rows_counter.lock().unwrap() += 1;
        });
        let _order = store.listen_order(move |_, _| {
            *order_counter.lock().unwrap() += 1;
        });

        store.set_order(vec![2, 1]);
        flush();
        assert_eq!(*rows.lock().unwrap(), 0, "строка не тронута");
        assert_eq!(*order.lock().unwrap(), 1, "порядок сменился");
        assert_eq!(store.get().order(), &[2, 1]);
    }

    /// An update that changes nothing writes nothing and wakes nobody.
    #[test]
    fn no_op_update_notifies_nobody() {
        let store = store();
        store.set_item(1, "one".into());

        let woken = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&woken);
        let _sub = store.listen_key(&1, move |_, _| {
            *counter.lock().unwrap() += 1;
        });

        store.update_item(1, |_| false);
        flush();
        assert_eq!(*woken.lock().unwrap(), 0);

        store.update_item(1, |value| {
            *value = "ONE".into();
            true
        });
        flush();
        assert_eq!(*woken.lock().unwrap(), 1);
    }

    /// Removal drops the item from both the map and the order.
    #[test]
    fn remove_drops_item_and_order_entry() {
        let store = store();
        store.set_item(1, "one".into());
        store.set_item(2, "two".into());

        store.remove_item(&1);
        let snapshot = store.get();
        assert_eq!(snapshot.get(&1), None);
        assert_eq!(snapshot.order(), &[2]);
        assert_eq!(snapshot.len(), 1);

        // Removing what is not there is a no-op, not a panic.
        store.remove_item(&1);
        assert_eq!(store.get().len(), 1);
    }

    /// iter() walks in order and skips keys whose item is missing.
    #[test]
    fn iter_follows_order() {
        let store = store();
        store.set_item(1, "one".into());
        store.set_item(2, "two".into());
        store.set_order(vec![2, 1]);

        let seen: Vec<String> = store.get().iter().map(|(_, value)| value.clone()).collect();
        assert_eq!(seen, vec!["two".to_string(), "one".to_string()]);
    }
}
