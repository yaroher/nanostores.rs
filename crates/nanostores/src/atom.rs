use crate::Subscription;
use crate::store::{Listener, StoreInner, StoreLike};
use std::sync::Arc;

pub fn atom<T>(init: T) -> Atom<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    Atom {
        inner: StoreInner::new(init),
    }
}

pub struct Atom<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    pub(crate) inner: Arc<StoreInner<T>>,
}

impl<T> Clone for Atom<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Atom<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    pub fn get(&self) -> T {
        self.inner.get()
    }

    pub fn set(&self, value: T) {
        self.inner.set_value(value, None, true);
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

impl<T> StoreLike for Atom<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
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
