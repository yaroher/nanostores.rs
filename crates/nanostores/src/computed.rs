use crate::Subscription;
use crate::scheduler;
use crate::store::{Listener, StoreInner, StoreLike};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type DeferredJob = Box<dyn FnOnce() + Send + 'static>;

// Per-thread: a notification stack on thread A must not defer (nor execute)
// recomputes triggered by stores notifying concurrently on thread B.
thread_local! {
    static NOTIFICATION_DEPTH: Cell<usize> = const { Cell::new(0) };
    static DEFERRED_RECOMPUTES: RefCell<VecDeque<DeferredJob>> =
        const { RefCell::new(VecDeque::new()) };
}

pub(crate) struct NotificationGuard;

pub(crate) fn notification_guard() -> NotificationGuard {
    NOTIFICATION_DEPTH.with(|depth| depth.set(depth.get() + 1));
    NotificationGuard
}

impl Drop for NotificationGuard {
    fn drop(&mut self) {
        let is_outermost = NOTIFICATION_DEPTH.with(|depth| {
            let value = depth.get();
            depth.set(value - 1);
            value == 1
        });
        if is_outermost {
            flush_deferred_recomputes();
        }
    }
}

fn defer_or_run(job: impl FnOnce() + Send + 'static) {
    let deferred = NOTIFICATION_DEPTH.with(|depth| depth.get() > 0);
    if deferred {
        DEFERRED_RECOMPUTES.with(|queue| queue.borrow_mut().push_back(Box::new(job)));
    } else {
        job();
    }
}

fn flush_deferred_recomputes() {
    loop {
        let job = DEFERRED_RECOMPUTES.with(|queue| queue.borrow_mut().pop_front());
        match job {
            Some(job) => job(),
            None => break,
        }
    }
}

pub trait ComputeDeps<F, T>: Clone + Send + Sync + 'static
where
    T: Clone + PartialEq + Send + Sync + 'static,
    F: Send + Sync + 'static,
{
    fn compute(&self, f: &F) -> T;
    fn listen_all(&self, callback: Arc<dyn Fn() + Send + Sync + 'static>) -> Vec<Subscription>;
}

macro_rules! impl_compute_deps {
    (($($idx:tt $store:ident),+)) => {
        impl<F, T, $($store,)+> ComputeDeps<F, T> for ($($store,)+)
        where
            T: Clone + PartialEq + Send + Sync + 'static,
            F: Fn($($store::Value),+) -> T + Send + Sync + 'static,
            $($store: StoreLike,)+
        {
            fn compute(&self, f: &F) -> T {
                f($(StoreLike::get(&self.$idx)),+)
            }

            fn listen_all(
                &self,
                callback: Arc<dyn Fn() + Send + Sync + 'static>,
            ) -> Vec<Subscription> {
                vec![
                    $({
                        let callback = Arc::clone(&callback);
                        self.$idx.listen_arc(Arc::new(move |_, _| callback()))
                    }),+
                ]
            }
        }
    };
}

impl_compute_deps!((0 A));
impl_compute_deps!((0 A, 1 B));
impl_compute_deps!((0 A, 1 B, 2 C));
impl_compute_deps!((0 A, 1 B, 2 C, 3 D));
impl_compute_deps!((0 A, 1 B, 2 C, 3 D, 4 E));
impl_compute_deps!((0 A, 1 B, 2 C, 3 D, 4 E, 5 G));
impl_compute_deps!((0 A, 1 B, 2 C, 3 D, 4 E, 5 G, 6 H));
impl_compute_deps!((0 A, 1 B, 2 C, 3 D, 4 E, 5 G, 6 H, 7 I));

pub fn computed<D, F, T>(deps: D, compute: F) -> Computed<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    Computed::new(deps, compute, false)
}

pub fn batched<D, F, T>(deps: D, compute: F) -> Computed<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    Computed::new(deps, compute, true)
}

pub type Batched<D, F, T> = Computed<D, F, T>;

pub struct Computed<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    pub(crate) inner: Arc<ComputedInner<D, F, T>>,
}

pub(crate) struct ComputedInner<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    pub(crate) store: Arc<StoreInner<T>>,
    deps: D,
    compute: F,
    dep_subscriptions: Mutex<Vec<Subscription>>,
    batched: bool,
    pending: AtomicBool,
}

impl<D, F, T> Clone for Computed<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<D, F, T> Computed<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    fn new(deps: D, compute: F, batched: bool) -> Self {
        let initial = deps.compute(&compute);
        let inner = Arc::new(ComputedInner {
            store: StoreInner::new(initial),
            deps,
            compute,
            dep_subscriptions: Mutex::new(Vec::new()),
            batched,
            pending: AtomicBool::new(false),
        });

        {
            let weak = Arc::downgrade(&inner);
            inner
                .store
                .add_start_hook(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.mount();
                    }
                })
                .detach();
        }

        {
            let weak = Arc::downgrade(&inner);
            inner
                .store
                .add_stop_hook(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.unmount();
                    }
                })
                .detach();
        }

        Self { inner }
    }

    pub fn get(&self) -> T {
        if self.inner.store.is_mounted() {
            self.inner.store.get()
        } else {
            let value = self.inner.deps.compute(&self.inner.compute);
            self.inner.store.replace_silently(value.clone());
            value
        }
    }

    pub fn listen<L>(&self, listener: L) -> Subscription
    where
        L: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        self.listen_arc(Arc::new(listener))
    }

    pub fn subscribe<L>(&self, listener: L) -> Subscription
    where
        L: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        self.subscribe_arc(Arc::new(listener))
    }
}

impl<D, F, T> ComputedInner<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    fn mount(self: &Arc<Self>) {
        {
            let dep_subscriptions = self
                .dep_subscriptions
                .lock()
                .expect("computed subscriptions poisoned");
            if !dep_subscriptions.is_empty() {
                return;
            }
        }

        let weak = Arc::downgrade(self);
        let callback = Arc::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.dependency_changed();
            }
        });
        let subscriptions = self.deps.listen_all(callback);
        *self
            .dep_subscriptions
            .lock()
            .expect("computed subscriptions poisoned") = subscriptions;
        self.recompute();
    }

    fn unmount(&self) {
        let subscriptions = std::mem::take(
            &mut *self
                .dep_subscriptions
                .lock()
                .expect("computed subscriptions poisoned"),
        );
        drop(subscriptions);
        self.pending.store(false, Ordering::SeqCst);
    }

    fn dependency_changed(self: &Arc<Self>) {
        if self.pending.swap(true, Ordering::SeqCst) {
            return;
        }

        let weak = Arc::downgrade(self);
        let recompute = move || {
            if let Some(inner) = weak.upgrade() {
                inner.pending.store(false, Ordering::SeqCst);
                if inner.store.is_mounted() {
                    inner.recompute();
                }
            }
        };

        if !self.batched {
            defer_or_run(recompute);
            return;
        }

        scheduler::schedule(move || {
            recompute();
        });
    }

    fn recompute(&self) {
        let value = self.deps.compute(&self.compute);
        self.store.set_value(value, None, false);
    }
}

impl<D, F, T> StoreLike for Computed<D, F, T>
where
    D: ComputeDeps<F, T>,
    F: Send + Sync + 'static,
    T: Clone + PartialEq + Send + Sync + 'static,
{
    type Value = T;

    fn get(&self) -> Self::Value {
        self.get()
    }

    fn listen_arc(&self, listener: Arc<Listener<Self::Value>>) -> Subscription {
        self.inner.store.add_listener(listener, false, None)
    }

    fn subscribe_arc(&self, listener: Arc<Listener<Self::Value>>) -> Subscription {
        self.inner.store.add_listener(listener, true, None)
    }
}
