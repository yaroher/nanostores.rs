use crate::Subscription;
use crate::atom::Atom;
use crate::computed::{ComputeDeps, Computed};
use crate::map::{MapStore, NanoMap};
use crate::store::{NotifyContext, SetContext, StoreLike};

pub trait Lifecycle: StoreLike {
    fn on_start<F>(&self, hook: F) -> Subscription
    where
        F: Fn() + Send + Sync + 'static;

    fn on_stop<F>(&self, hook: F) -> Subscription
    where
        F: Fn() + Send + Sync + 'static;

    fn on_mount<F, C>(&self, hook: F) -> Subscription
    where
        F: Fn() -> C + Send + Sync + 'static,
        C: FnOnce() + Send + 'static;

    fn on_set<F>(&self, hook: F) -> Subscription
    where
        F: Fn(&mut SetContext<Self::Value>) + Send + Sync + 'static;

    fn on_notify<F>(&self, hook: F) -> Subscription
    where
        F: Fn(&mut NotifyContext<Self::Value>) + Send + Sync + 'static;
}

pub fn on_start<S, F>(store: &S, hook: F) -> Subscription
where
    S: Lifecycle,
    F: Fn() + Send + Sync + 'static,
{
    store.on_start(hook)
}

pub fn on_stop<S, F>(store: &S, hook: F) -> Subscription
where
    S: Lifecycle,
    F: Fn() + Send + Sync + 'static,
{
    store.on_stop(hook)
}

pub fn on_mount<S, F, C>(store: &S, hook: F) -> Subscription
where
    S: Lifecycle,
    F: Fn() -> C + Send + Sync + 'static,
    C: FnOnce() + Send + 'static,
{
    store.on_mount(hook)
}

pub fn on_set<S, F>(store: &S, hook: F) -> Subscription
where
    S: Lifecycle,
    F: Fn(&mut SetContext<S::Value>) + Send + Sync + 'static,
{
    store.on_set(hook)
}

pub fn on_notify<S, F>(store: &S, hook: F) -> Subscription
where
    S: Lifecycle,
    F: Fn(&mut NotifyContext<S::Value>) + Send + Sync + 'static,
{
    store.on_notify(hook)
}

impl<T> Lifecycle for Atom<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    fn on_start<F>(&self, hook: F) -> Subscription
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.add_start_hook(hook)
    }

    fn on_stop<F>(&self, hook: F) -> Subscription
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.add_stop_hook(hook)
    }

    fn on_mount<F, C>(&self, hook: F) -> Subscription
    where
        F: Fn() -> C + Send + Sync + 'static,
        C: FnOnce() + Send + 'static,
    {
        self.inner.add_mount_hook(move || Subscription::new(hook()))
    }

    fn on_set<F>(&self, hook: F) -> Subscription
    where
        F: Fn(&mut SetContext<Self::Value>) + Send + Sync + 'static,
    {
        self.inner.add_set_hook(hook)
    }

    fn on_notify<F>(&self, hook: F) -> Subscription
    where
        F: Fn(&mut NotifyContext<Self::Value>) + Send + Sync + 'static,
    {
        self.inner.add_notify_hook(hook)
    }
}

impl<T> Lifecycle for MapStore<T>
where
    T: NanoMap + Clone + PartialEq + Send + Sync + 'static,
{
    fn on_start<F>(&self, hook: F) -> Subscription
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.add_start_hook(hook)
    }

    fn on_stop<F>(&self, hook: F) -> Subscription
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.inner.add_stop_hook(hook)
    }

    fn on_mount<F, C>(&self, hook: F) -> Subscription
    where
        F: Fn() -> C + Send + Sync + 'static,
        C: FnOnce() + Send + 'static,
    {
        self.inner.add_mount_hook(move || Subscription::new(hook()))
    }

    fn on_set<F>(&self, hook: F) -> Subscription
    where
        F: Fn(&mut SetContext<Self::Value>) + Send + Sync + 'static,
    {
        self.inner.add_set_hook(hook)
    }

    fn on_notify<F>(&self, hook: F) -> Subscription
    where
        F: Fn(&mut NotifyContext<Self::Value>) + Send + Sync + 'static,
    {
        self.inner.add_notify_hook(hook)
    }
}

impl<D, F, T> Lifecycle for Computed<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    fn on_start<H>(&self, hook: H) -> Subscription
    where
        H: Fn() + Send + Sync + 'static,
    {
        self.inner.store.add_start_hook(hook)
    }

    fn on_stop<H>(&self, hook: H) -> Subscription
    where
        H: Fn() + Send + Sync + 'static,
    {
        self.inner.store.add_stop_hook(hook)
    }

    fn on_mount<H, C>(&self, hook: H) -> Subscription
    where
        H: Fn() -> C + Send + Sync + 'static,
        C: FnOnce() + Send + 'static,
    {
        self.inner
            .store
            .add_mount_hook(move || Subscription::new(hook()))
    }

    fn on_set<H>(&self, hook: H) -> Subscription
    where
        H: Fn(&mut SetContext<Self::Value>) + Send + Sync + 'static,
    {
        self.inner.store.add_set_hook(hook)
    }

    fn on_notify<H>(&self, hook: H) -> Subscription
    where
        H: Fn(&mut NotifyContext<Self::Value>) + Send + Sync + 'static,
    {
        self.inner.store.add_notify_hook(hook)
    }
}
