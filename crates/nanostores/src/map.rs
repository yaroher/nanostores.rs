use crate::Subscription;
use crate::error::KeyError;
use crate::store::{Listener, StoreInner, StoreLike};
use serde::{Deserializer, Serializer};
use std::sync::Arc;

pub trait NanoMap: Sized {
    const KEYS: &'static [&'static str];

    fn set_field<'de, D>(&mut self, key: &str, value: D) -> Result<(), KeyError>
    where
        D: Deserializer<'de>;

    fn get_field<S>(&self, key: &str, ser: S) -> Result<S::Ok, KeyError>
    where
        S: Serializer;
}

pub fn map<T>(init: T) -> MapStore<T>
where
    T: NanoMap + Clone + PartialEq + Send + Sync + 'static,
{
    MapStore {
        inner: StoreInner::new(init),
    }
}

pub struct MapStore<T>
where
    T: NanoMap + Clone + PartialEq + Send + Sync + 'static,
{
    pub(crate) inner: Arc<StoreInner<T>>,
}

impl<T> Clone for MapStore<T>
where
    T: NanoMap + Clone + PartialEq + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> MapStore<T>
where
    T: NanoMap + Clone + PartialEq + Send + Sync + 'static,
{
    pub fn get(&self) -> T {
        self.inner.get()
    }

    pub fn set(&self, value: T) {
        self.inner.set_value(value, None, true);
    }

    pub fn set_key<'de, D>(&self, key: &str, value: D) -> Result<(), KeyError>
    where
        D: Deserializer<'de>,
    {
        let mut next = self.inner.get();
        next.set_field(key, value)?;
        self.inner.set_value(next, Some(key.to_owned()), true);
        Ok(())
    }

    pub fn set_field_by<F>(&self, key: &'static str, set_field: F)
    where
        F: FnOnce(&mut T) -> bool,
    {
        let mut next = self.inner.get();
        if set_field(&mut next) {
            self.inner.set_value(next, Some(key.to_owned()), true);
        }
    }

    pub fn update<F>(&self, update: F)
    where
        F: FnOnce(&mut T),
    {
        let mut next = self.inner.get();
        update(&mut next);
        self.inner.set_value(next, None, true);
    }

    pub fn listen_keys<F>(&self, keys: &[&str], listener: F) -> Subscription
    where
        F: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        let keys = keys
            .iter()
            .map(|key| (*key).to_owned())
            .collect::<Vec<_>>()
            .into();
        self.inner
            .add_listener(Arc::new(listener), false, Some(keys))
    }

    pub fn listen<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        self.listen_arc(Arc::new(listener))
    }

    pub fn subscribe<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        self.subscribe_arc(Arc::new(listener))
    }
}

impl<T> StoreLike for MapStore<T>
where
    T: NanoMap + Clone + PartialEq + Send + Sync + 'static,
{
    type Value = T;

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
