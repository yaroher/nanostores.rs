use crate::Subscription;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

pub type Listener<T> = dyn Fn(&T, Option<&str>) + Send + Sync + 'static;

/// Which listeners a notification reaches.
///
/// `All` wakes everyone (listeners see `changed_key: None`). `Keys` wakes
/// only listeners filtered on one of the keys — a collection insert or
/// removal announces both the item's key and `ORDER_KEY`, so row listeners
/// and the order listener each hear their own part. Listeners see the
/// first key as `changed_key`.
#[derive(Clone, Debug)]
pub(crate) enum NotifyScope {
    All,
    Keys(Vec<String>),
}

impl NotifyScope {
    fn listener_changed_key(&self) -> Option<&str> {
        match self {
            NotifyScope::All => None,
            NotifyScope::Keys(keys) => keys.first().map(String::as_str),
        }
    }
}

type StartHook = dyn Fn() + Send + Sync + 'static;
type StopHook = dyn Fn() + Send + Sync + 'static;
type MountHook = dyn Fn() -> Subscription + Send + Sync + 'static;
type SetHook<T> = dyn Fn(&mut SetContext<T>) + Send + Sync + 'static;
type NotifyHook<T> = dyn Fn(&mut NotifyContext<T>) + Send + Sync + 'static;

#[derive(Debug, Clone)]
pub struct ChangeContext<T> {
    value: T,
    changed_key: Option<String>,
    aborted: bool,
}

impl<T> ChangeContext<T> {
    pub(crate) fn new(value: T, changed_key: Option<String>) -> Self {
        Self {
            value,
            changed_key,
            aborted: false,
        }
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn value_mut(&mut self) -> &mut T {
        &mut self.value
    }

    pub fn changed_key(&self) -> Option<&str> {
        self.changed_key.as_deref()
    }

    pub fn abort(&mut self) {
        self.aborted = true;
    }

    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    pub(crate) fn into_parts(self) -> (T, bool) {
        (self.value, self.aborted)
    }
}

pub type SetContext<T> = ChangeContext<T>;
pub type NotifyContext<T> = ChangeContext<T>;

#[derive(Clone)]
struct ListenerEntry<T> {
    id: usize,
    keys: Option<Arc<[String]>>,
    listener: Arc<Listener<T>>,
}

struct State<T> {
    next_id: usize,
    listeners: Vec<ListenerEntry<T>>,
    start_hooks: Vec<(usize, Arc<StartHook>)>,
    stop_hooks: Vec<(usize, Arc<StopHook>)>,
    mount_hooks: Vec<(usize, Arc<MountHook>)>,
    mount_cleanups: Vec<(usize, Subscription)>,
    set_hooks: Vec<(usize, Arc<SetHook<T>>)>,
    notify_hooks: Vec<(usize, Arc<NotifyHook<T>>)>,
}

impl<T> Default for State<T> {
    fn default() -> Self {
        Self {
            next_id: 0,
            listeners: Vec::new(),
            start_hooks: Vec::new(),
            stop_hooks: Vec::new(),
            mount_hooks: Vec::new(),
            mount_cleanups: Vec::new(),
            set_hooks: Vec::new(),
            notify_hooks: Vec::new(),
        }
    }
}

pub(crate) struct StoreInner<T> {
    value: RwLock<T>,
    state: Mutex<State<T>>,
    // Concurrent writers race to notify; without ordering a slower thread
    // could deliver an older value AFTER a newer one. Writes get a monotonic
    // seq under the value lock, notifications go through a queue drained by
    // a single thread at a time, and anything older than the last delivered
    // seq is dropped as superseded.
    write_seq: AtomicU64,
    delivered_seq: AtomicU64,
    notify_queue: Mutex<VecDeque<QueuedNotification<T>>>,
    draining: AtomicBool,
}

struct QueuedNotification<T> {
    seq: u64,
    value: T,
    scope: NotifyScope,
}

impl<T> StoreInner<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    pub(crate) fn new(value: T) -> Arc<Self> {
        Arc::new(Self {
            value: RwLock::new(value),
            state: Mutex::new(State::default()),
            write_seq: AtomicU64::new(0),
            delivered_seq: AtomicU64::new(0),
            notify_queue: Mutex::new(VecDeque::new()),
            draining: AtomicBool::new(false),
        })
    }

    pub(crate) fn get(&self) -> T {
        self.value.read().expect("store value poisoned").clone()
    }

    pub(crate) fn replace_silently(&self, value: T) {
        *self.value.write().expect("store value poisoned") = value;
    }

    pub(crate) fn listener_count(&self) -> usize {
        self.state
            .lock()
            .expect("store state poisoned")
            .listeners
            .len()
    }

    pub(crate) fn is_mounted(&self) -> bool {
        self.listener_count() > 0
    }

    pub(crate) fn add_listener(
        self: &Arc<Self>,
        listener: Arc<Listener<T>>,
        immediate: bool,
        keys: Option<Arc<[String]>>,
    ) -> Subscription {
        let (id, start_hooks, mount_hooks) = {
            let mut state = self.state.lock().expect("store state poisoned");
            let id = state.next_id;
            state.next_id += 1;
            let should_start = state.listeners.is_empty();
            state.listeners.push(ListenerEntry { id, keys, listener });

            if should_start {
                (
                    id,
                    state
                        .start_hooks
                        .iter()
                        .map(|(_, hook)| Arc::clone(hook))
                        .collect::<Vec<_>>(),
                    state
                        .mount_hooks
                        .iter()
                        .map(|(hook_id, hook)| (*hook_id, Arc::clone(hook)))
                        .collect::<Vec<_>>(),
                )
            } else {
                (id, Vec::new(), Vec::new())
            }
        };

        for hook in start_hooks {
            hook();
        }

        if !mount_hooks.is_empty() {
            let cleanups = mount_hooks
                .into_iter()
                .map(|(hook_id, hook)| (hook_id, hook()))
                .collect::<Vec<_>>();
            self.state
                .lock()
                .expect("store state poisoned")
                .mount_cleanups
                .extend(cleanups);
        }

        if immediate {
            let value = self.get();
            self.invoke_listener(id, &value, None);
        }

        let weak = Arc::downgrade(self);
        Subscription::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.remove_listener(id);
            }
        })
    }

    fn invoke_listener(&self, id: usize, value: &T, changed_key: Option<&str>) {
        let listener = {
            let state = self.state.lock().expect("store state poisoned");
            state
                .listeners
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| Arc::clone(&entry.listener))
        };

        if let Some(listener) = listener {
            listener(value, changed_key);
        }
    }

    fn remove_listener(&self, id: usize) {
        let (cleanups, stop_hooks) = {
            let mut state = self.state.lock().expect("store state poisoned");
            let was_mounted = !state.listeners.is_empty();
            state.listeners.retain(|entry| entry.id != id);

            if was_mounted && state.listeners.is_empty() {
                let cleanups = std::mem::take(&mut state.mount_cleanups);
                let stop_hooks = state
                    .stop_hooks
                    .iter()
                    .map(|(_, hook)| Arc::clone(hook))
                    .collect::<Vec<_>>();
                (cleanups, stop_hooks)
            } else {
                (Vec::new(), Vec::new())
            }
        };

        drop(cleanups);
        for hook in stop_hooks {
            hook();
        }
    }

    pub(crate) fn set_value(
        &self,
        value: T,
        changed_key: Option<String>,
        run_set_hooks: bool,
    ) -> bool {
        let scope = match changed_key {
            None => NotifyScope::All,
            Some(key) => NotifyScope::Keys(vec![key]),
        };
        self.commit(value, scope, run_set_hooks)
    }

    /// The full write path: set hooks may transform or abort, the new value
    /// is compared and stored, notify hooks may transform again, and the
    /// notification reaches the listeners selected by `scope`.
    pub(crate) fn commit(&self, value: T, scope: NotifyScope, run_set_hooks: bool) -> bool {
        let mut next = value;

        if run_set_hooks {
            let mut context =
                SetContext::new(next, scope.listener_changed_key().map(str::to_owned));
            for hook in self.snapshot_set_hooks() {
                hook(&mut context);
                if context.is_aborted() {
                    return false;
                }
            }
            let (value, aborted) = context.into_parts();
            if aborted {
                return false;
            }
            next = value;
        }

        let seq = {
            let mut current = self.value.write().expect("store value poisoned");
            if *current == next {
                return false;
            }
            *current = next.clone();
            self.write_seq.fetch_add(1, Ordering::SeqCst) + 1
        };

        let mut notify_context =
            NotifyContext::new(next, scope.listener_changed_key().map(str::to_owned));
        for hook in self.snapshot_notify_hooks() {
            hook(&mut notify_context);
            if notify_context.is_aborted() {
                return true;
            }
        }

        let (notify_value, aborted) = notify_context.into_parts();
        if aborted {
            return true;
        }

        {
            let mut current = self.value.write().expect("store value poisoned");
            if *current != notify_value {
                *current = notify_value.clone();
            }
        }

        self.notify_listeners(seq, notify_value, scope);
        true
    }

    /// In-place write path: mutates the stored value under the write lock
    /// (no pre-clone, no deep equality passes — the closure itself decides
    /// whether anything changed) and notifies with a single snapshot clone.
    ///
    /// Returns `Ok(handled)` when the call was handled here, or `Err` with
    /// the untouched closure when set or notify hooks are registered and
    /// the caller must run the copy-on-write [`StoreInner::commit`] path
    /// instead (where those hooks execute). The closure is handed back so
    /// an `FnOnce` is not lost to a declined fast path.
    pub(crate) fn mutate_in_place<F>(
        &self,
        scope: NotifyScope,
        mutate: F,
    ) -> Result<bool, impl FnOnce(&mut T) -> bool>
    where
        F: FnOnce(&mut T) -> bool,
    {
        if self.has_set_or_notify_hooks() {
            return Err(mutate);
        }

        let (seq, snapshot) = {
            let mut current = self.value.write().expect("store value poisoned");
            if !mutate(&mut current) {
                return Ok(true);
            }
            let snapshot = current.clone();
            (self.write_seq.fetch_add(1, Ordering::SeqCst) + 1, snapshot)
        };

        self.notify_listeners(seq, snapshot, scope);
        Ok(true)
    }

    /// Run `read` against the stored value under the read lock — a cheap
    /// existence or content peek without cloning the value.
    pub(crate) fn read_value<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        let value = self.value.read().expect("store value poisoned");
        read(&value)
    }

    fn has_set_or_notify_hooks(&self) -> bool {
        let state = self.state.lock().expect("store state poisoned");
        !state.set_hooks.is_empty() || !state.notify_hooks.is_empty()
    }

    fn notify_listeners(&self, seq: u64, value: T, scope: NotifyScope) {
        self.notify_queue
            .lock()
            .expect("store notify queue poisoned")
            .push_back(QueuedNotification { seq, value, scope });

        // Single drainer: whichever thread wins delivers everything queued
        // (including notifications enqueued by other threads or reentrant
        // sets from listeners), in write order. Others just enqueue.
        if self.draining.swap(true, Ordering::SeqCst) {
            return;
        }

        loop {
            loop {
                let item = self
                    .notify_queue
                    .lock()
                    .expect("store notify queue poisoned")
                    .pop_front();
                let Some(item) = item else { break };

                // Superseded by an already-delivered newer write.
                if item.seq <= self.delivered_seq.load(Ordering::SeqCst) {
                    continue;
                }
                self.delivered_seq.store(item.seq, Ordering::SeqCst);

                let changed_key = item.scope.listener_changed_key();
                let listeners = {
                    let state = self.state.lock().expect("store state poisoned");
                    state
                        .listeners
                        .iter()
                        .filter(|entry| listener_matches(entry.keys.as_deref(), &item.scope))
                        .map(|entry| Arc::clone(&entry.listener))
                        .collect::<Vec<_>>()
                };

                let _notification = crate::computed::notification_guard();
                for listener in listeners {
                    listener(&item.value, changed_key);
                }
            }

            self.draining.store(false, Ordering::SeqCst);
            // A racer may have enqueued between the final pop and the flag
            // reset; reclaim drain rights unless someone else already did.
            if self
                .notify_queue
                .lock()
                .expect("store notify queue poisoned")
                .is_empty()
                || self.draining.swap(true, Ordering::SeqCst)
            {
                break;
            }
        }
    }

    fn snapshot_set_hooks(&self) -> Vec<Arc<SetHook<T>>> {
        self.state
            .lock()
            .expect("store state poisoned")
            .set_hooks
            .iter()
            .map(|(_, hook)| Arc::clone(hook))
            .collect()
    }

    fn snapshot_notify_hooks(&self) -> Vec<Arc<NotifyHook<T>>> {
        self.state
            .lock()
            .expect("store state poisoned")
            .notify_hooks
            .iter()
            .map(|(_, hook)| Arc::clone(hook))
            .collect()
    }

    pub(crate) fn add_start_hook(
        self: &Arc<Self>,
        hook: impl Fn() + Send + Sync + 'static,
    ) -> Subscription {
        self.add_hook(|state, id, hook| state.start_hooks.push((id, hook)), hook)
    }

    pub(crate) fn add_stop_hook(
        self: &Arc<Self>,
        hook: impl Fn() + Send + Sync + 'static,
    ) -> Subscription {
        self.add_hook(|state, id, hook| state.stop_hooks.push((id, hook)), hook)
    }

    pub(crate) fn add_mount_hook(
        self: &Arc<Self>,
        hook: impl Fn() -> Subscription + Send + Sync + 'static,
    ) -> Subscription {
        let id = {
            let mut state = self.state.lock().expect("store state poisoned");
            let id = state.next_id;
            state.next_id += 1;
            state.mount_hooks.push((id, Arc::new(hook)));
            id
        };

        let weak = Arc::downgrade(self);
        Subscription::new(move || {
            if let Some(inner) = weak.upgrade() {
                let active = {
                    let mut state = inner.state.lock().expect("store state poisoned");
                    state.mount_hooks.retain(|(hook_id, _)| *hook_id != id);
                    drain_matching(&mut state.mount_cleanups, id)
                };
                drop(active);
            }
        })
    }

    pub(crate) fn add_set_hook(
        self: &Arc<Self>,
        hook: impl Fn(&mut SetContext<T>) + Send + Sync + 'static,
    ) -> Subscription {
        self.add_hook(|state, id, hook| state.set_hooks.push((id, hook)), hook)
    }

    pub(crate) fn add_notify_hook(
        self: &Arc<Self>,
        hook: impl Fn(&mut NotifyContext<T>) + Send + Sync + 'static,
    ) -> Subscription {
        self.add_hook(|state, id, hook| state.notify_hooks.push((id, hook)), hook)
    }

    fn add_hook<H>(
        self: &Arc<Self>,
        push: impl FnOnce(&mut State<T>, usize, Arc<H>),
        hook: impl Into<Arc<H>>,
    ) -> Subscription
    where
        H: ?Sized + Send + Sync + 'static,
    {
        let id = {
            let mut state = self.state.lock().expect("store state poisoned");
            let id = state.next_id;
            state.next_id += 1;
            push(&mut state, id, hook.into());
            id
        };

        hook_subscription(self, move |state| {
            state.start_hooks.retain(|(hook_id, _)| *hook_id != id);
            state.stop_hooks.retain(|(hook_id, _)| *hook_id != id);
            state.set_hooks.retain(|(hook_id, _)| *hook_id != id);
            state.notify_hooks.retain(|(hook_id, _)| *hook_id != id);
        })
    }
}

fn listener_matches(keys: Option<&[String]>, scope: &NotifyScope) -> bool {
    match (keys, scope) {
        (None, _) => true,
        (Some(_), NotifyScope::All) => true,
        (Some(listener_keys), NotifyScope::Keys(changed)) => changed
            .iter()
            .any(|changed_key| listener_keys.iter().any(|key| key == changed_key)),
    }
}

fn hook_subscription<T>(
    inner: &Arc<StoreInner<T>>,
    remove: impl FnOnce(&mut State<T>) + Send + 'static,
) -> Subscription
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let weak: Weak<StoreInner<T>> = Arc::downgrade(inner);
    Subscription::new(move || {
        if let Some(inner) = weak.upgrade() {
            let mut state = inner.state.lock().expect("store state poisoned");
            remove(&mut state);
        }
    })
}

fn drain_matching<T>(items: &mut Vec<(usize, T)>, id: usize) -> Vec<T> {
    let mut drained = Vec::new();
    let mut kept = Vec::with_capacity(items.len());
    for (item_id, item) in items.drain(..) {
        if item_id == id {
            drained.push(item);
        } else {
            kept.push((item_id, item));
        }
    }
    *items = kept;
    drained
}

pub trait StoreLike: Clone + Send + Sync + 'static {
    type Value: Clone + PartialEq + Send + Sync + 'static;

    fn get(&self) -> Self::Value;
    fn listen_arc(&self, listener: Arc<Listener<Self::Value>>) -> Subscription;
    fn subscribe_arc(&self, listener: Arc<Listener<Self::Value>>) -> Subscription;
}

pub trait ReadableStore<T>: StoreLike<Value = T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    fn get(&self) -> T {
        StoreLike::get(self)
    }

    fn listen<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        self.listen_arc(Arc::new(listener))
    }

    fn subscribe<F>(&self, listener: F) -> Subscription
    where
        F: Fn(&T, Option<&str>) + Send + Sync + 'static,
    {
        self.subscribe_arc(Arc::new(listener))
    }
}

impl<S, T> ReadableStore<T> for S
where
    S: StoreLike<Value = T>,
    T: Clone + PartialEq + Send + Sync + 'static,
{
}
