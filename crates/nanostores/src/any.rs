use crate::Subscription;
use crate::store::{Listener, StoreLike};
use std::sync::Arc;

trait ErasedStore<T>: Send + Sync {
    fn get(&self) -> T;
    fn listen_arc(&self, listener: Arc<Listener<T>>) -> Subscription;
    fn subscribe_arc(&self, listener: Arc<Listener<T>>) -> Subscription;
}

impl<S, T> ErasedStore<T> for S
where
    S: StoreLike<Value = T>,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    fn get(&self) -> T {
        StoreLike::get(self)
    }

    fn listen_arc(&self, listener: Arc<Listener<T>>) -> Subscription {
        StoreLike::listen_arc(self, listener)
    }

    fn subscribe_arc(&self, listener: Arc<Listener<T>>) -> Subscription {
        StoreLike::subscribe_arc(self, listener)
    }
}

/// Type-erased read-only store handle: exposes any `StoreLike` (notably
/// `Computed`, whose concrete type carries dependency and function generics)
/// as a value parameterized only by `T`. Makes derived stores storable in
/// statics and struct fields without naming their full type.
pub struct AnyStore<T>(Arc<dyn ErasedStore<T>>);

impl<T> Clone for AnyStore<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> AnyStore<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    pub fn new(store: impl StoreLike<Value = T>) -> Self {
        Self(Arc::new(store))
    }

    pub fn get(&self) -> T {
        self.0.get()
    }

    pub fn listen<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        self.0.listen_arc(Arc::new(listener))
    }

    pub fn subscribe<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        self.0.subscribe_arc(Arc::new(listener))
    }
}

impl<T> StoreLike for AnyStore<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    type Value = T;

    fn get(&self) -> Self::Value {
        self.0.get()
    }

    fn listen_arc(&self, listener: Arc<Listener<Self::Value>>) -> Subscription {
        self.0.listen_arc(listener)
    }

    fn subscribe_arc(&self, listener: Arc<Listener<Self::Value>>) -> Subscription {
        self.0.subscribe_arc(listener)
    }
}
