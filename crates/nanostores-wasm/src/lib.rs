#![cfg(target_arch = "wasm32")]

use js_sys::{Function, Object, Reflect};
use nanostores::{
    Atom, MapStore, NanoMap, Scheduler, StoreLike, Subscription, set_scheduler_if_unset,
};
use send_wrapper::SendWrapper;
use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Display;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, Once};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn error(value: &JsValue);
}

trait ErasedAtom: Send + Sync {
    fn get(&self) -> Result<JsValue, JsValue>;
    fn set(&self, value: JsValue) -> Result<(), JsValue>;
    fn subscribe(&self, callback: Function) -> Subscription;
}

trait ErasedMap: Send + Sync {
    fn get(&self) -> Result<JsValue, JsValue>;
    fn set(&self, value: JsValue) -> Result<(), JsValue>;
    fn set_key(&self, key: String, value: JsValue) -> Result<(), JsValue>;
    fn subscribe(&self, callback: Function) -> Subscription;
}

struct WritableAtomProjection<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    store: Atom<T>,
}

struct ReadableAtomProjection<S, T> {
    store: S,
    _value: PhantomData<T>,
}

struct MapProjection<T>
where
    T: NanoMap + Clone + PartialEq + Send + Sync + 'static,
{
    store: MapStore<T>,
}

impl<T> ErasedAtom for WritableAtomProjection<T>
where
    T: Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
{
    fn get(&self) -> Result<JsValue, JsValue> {
        to_js(&self.store.get())
    }

    fn set(&self, value: JsValue) -> Result<(), JsValue> {
        self.store.set(from_js(value)?);
        Ok(())
    }

    fn subscribe(&self, callback: Function) -> Subscription {
        let callback = callback_cell(callback);
        self.store
            .listen(move |value, _| call_atom(&callback, value))
    }
}

impl<S, T> ErasedAtom for ReadableAtomProjection<S, T>
where
    S: StoreLike<Value = T>,
    T: Serialize + Clone + PartialEq + Send + Sync + 'static,
{
    fn get(&self) -> Result<JsValue, JsValue> {
        to_js(&self.store.get())
    }

    fn set(&self, _value: JsValue) -> Result<(), JsValue> {
        Err(js_error("store is read-only"))
    }

    fn subscribe(&self, callback: Function) -> Subscription {
        let callback = callback_cell(callback);
        self.store
            .listen_arc(Arc::new(move |value, _| call_atom(&callback, value)))
    }
}

impl<T> ErasedMap for MapProjection<T>
where
    T: NanoMap + Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
{
    fn get(&self) -> Result<JsValue, JsValue> {
        to_js(&self.store.get())
    }

    fn set(&self, value: JsValue) -> Result<(), JsValue> {
        self.store.set(from_js(value)?);
        Ok(())
    }

    fn set_key(&self, key: String, value: JsValue) -> Result<(), JsValue> {
        let deserializer = serde_wasm_bindgen::Deserializer::from(value);
        self.store.set_key(&key, deserializer).map_err(js_error)?;
        Ok(())
    }

    fn subscribe(&self, callback: Function) -> Subscription {
        let callback = callback_cell(callback);
        self.store
            .listen(move |value, changed_key| call_map(&callback, value, changed_key))
    }
}

#[wasm_bindgen]
pub struct AtomHandle {
    inner: Arc<dyn ErasedAtom>,
}

impl AtomHandle {
    pub fn from_atom<T>(store: &Atom<T>) -> Self
    where
        T: Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
    {
        install_scheduler();
        Self {
            inner: Arc::new(WritableAtomProjection {
                store: store.clone(),
            }),
        }
    }

    pub fn from_readable<S, T>(store: &S) -> Self
    where
        S: StoreLike<Value = T>,
        T: Serialize + Clone + PartialEq + Send + Sync + 'static,
    {
        install_scheduler();
        Self {
            inner: Arc::new(ReadableAtomProjection {
                store: store.clone(),
                _value: PhantomData,
            }),
        }
    }
}

#[wasm_bindgen]
impl AtomHandle {
    pub fn get(&self) -> Result<JsValue, JsValue> {
        self.inner.get()
    }

    pub fn set(&self, value: JsValue) -> Result<(), JsValue> {
        self.inner.set(value)
    }

    pub fn subscribe(&self, callback: Function) -> SubscriptionHandle {
        SubscriptionHandle::new(self.inner.subscribe(callback))
    }
}

#[wasm_bindgen]
pub struct MapHandle {
    inner: Arc<dyn ErasedMap>,
}

impl MapHandle {
    pub fn from_map<T>(store: &MapStore<T>) -> Self
    where
        T: NanoMap + Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
    {
        install_scheduler();
        Self {
            inner: Arc::new(MapProjection {
                store: store.clone(),
            }),
        }
    }
}

#[wasm_bindgen]
impl MapHandle {
    pub fn get(&self) -> Result<JsValue, JsValue> {
        self.inner.get()
    }

    pub fn set(&self, value: JsValue) -> Result<(), JsValue> {
        self.inner.set(value)
    }

    #[wasm_bindgen(js_name = setKey)]
    pub fn set_key(&self, key: String, value: JsValue) -> Result<(), JsValue> {
        self.inner.set_key(key, value)
    }

    pub fn subscribe(&self, callback: Function) -> SubscriptionHandle {
        SubscriptionHandle::new(self.inner.subscribe(callback))
    }
}

#[wasm_bindgen]
pub struct SubscriptionHandle {
    subscription: Option<Subscription>,
}

impl SubscriptionHandle {
    fn new(subscription: Subscription) -> Self {
        Self {
            subscription: Some(subscription),
        }
    }
}

#[wasm_bindgen]
impl SubscriptionHandle {
    pub fn unsubscribe(&mut self) {
        self.subscription.take();
    }
}

pub struct ExportBuilder {
    object: Object,
}

pub fn export() -> ExportBuilder {
    install_scheduler();
    ExportBuilder {
        object: Object::new(),
    }
}

impl ExportBuilder {
    pub fn atom<T>(self, name: &str, store: &Atom<T>) -> Self
    where
        T: Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
    {
        self.set(name, AtomHandle::from_atom(store));
        self
    }

    pub fn readable<S, T>(self, name: &str, store: &S) -> Self
    where
        S: StoreLike<Value = T>,
        T: Serialize + Clone + PartialEq + Send + Sync + 'static,
    {
        self.set(name, AtomHandle::from_readable(store));
        self
    }

    pub fn map<T>(self, name: &str, store: &MapStore<T>) -> Self
    where
        T: NanoMap + Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + 'static,
    {
        self.set(name, MapHandle::from_map(store));
        self
    }

    pub fn build(self) -> JsValue {
        self.object.into()
    }

    fn set(&self, name: &str, handle: impl Into<JsValue>) {
        let ok = Reflect::set(&self.object, &JsValue::from_str(name), &handle.into())
            .expect("setting export object property should not throw");
        debug_assert!(ok);
    }
}

#[doc(hidden)]
pub fn __nanostores_wasm_store_kinds(entries: &[(&str, &str)]) -> JsValue {
    let object = Object::new();
    for (name, kind) in entries {
        let ok = Reflect::set(&object, &JsValue::from_str(name), &JsValue::from_str(kind))
            .expect("setting store kind property should not throw");
        debug_assert!(ok);
    }
    object.into()
}

/// Install the browser microtask scheduler for `batched` stores. Idempotent,
/// and never overrides a scheduler the application set explicitly via
/// `nanostores::set_scheduler`.
///
/// Every bridge entry point (`export()`, handle constructors) calls this, so
/// explicit initialization is only needed when a Rust wasm app uses `batched`
/// from the core crate without creating any bridge handles first.
pub fn install_scheduler() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        set_scheduler_if_unset(MicrotaskScheduler);
    });
}

struct MicrotaskScheduler;

impl Scheduler for MicrotaskScheduler {
    fn schedule(&self, flush: Box<dyn FnOnce() + Send>) {
        let callback = Closure::once_into_js(flush);
        let queue_microtask = Reflect::get(&js_sys::global(), &JsValue::from_str("queueMicrotask"))
            .ok()
            .and_then(|value| value.dyn_into::<Function>().ok());

        if let Some(queue_microtask) = queue_microtask {
            if let Err(err) = queue_microtask.call1(&JsValue::NULL, &callback) {
                error(&err);
            }
        } else {
            let flush = callback
                .dyn_into::<Function>()
                .expect("Closure::once_into_js creates a function");
            if let Err(err) = flush.call0(&JsValue::NULL) {
                error(&err);
            }
        }
    }
}

fn callback_cell(callback: Function) -> Arc<Mutex<SendWrapper<Function>>> {
    Arc::new(Mutex::new(SendWrapper::new(callback)))
}

fn call_atom<T>(callback: &Arc<Mutex<SendWrapper<Function>>>, value: &T)
where
    T: Serialize,
{
    match to_js(value) {
        Ok(value) => call_locked(callback, |callback| callback.call1(&JsValue::NULL, &value)),
        Err(err) => error(&err),
    }
}

fn call_map<T>(callback: &Arc<Mutex<SendWrapper<Function>>>, value: &T, changed_key: Option<&str>)
where
    T: Serialize,
{
    let value = match to_js(value) {
        Ok(value) => value,
        Err(err) => {
            error(&err);
            return;
        }
    };
    let changed_key = changed_key
        .map(JsValue::from_str)
        .unwrap_or(JsValue::UNDEFINED);

    call_locked(callback, |callback| {
        callback.call2(&JsValue::NULL, &value, &changed_key)
    });
}

fn call_locked(
    callback: &Arc<Mutex<SendWrapper<Function>>>,
    call: impl FnOnce(&Function) -> Result<JsValue, JsValue>,
) {
    let callback = callback.lock().expect("JS callback poisoned");
    if let Err(err) = call(&callback) {
        error(&err);
    }
}

fn to_js<T>(value: &T) -> Result<JsValue, JsValue>
where
    T: Serialize,
{
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    value.serialize(&serializer).map_err(js_error)
}

fn from_js<T>(value: JsValue) -> Result<T, JsValue>
where
    T: DeserializeOwned,
{
    serde_wasm_bindgen::from_value(value).map_err(js_error)
}

fn js_error(error: impl Display) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}

/// Define application stores and export them in one block: generates a
/// `LazyLock` static plus a `fn name() -> &'static ...` accessor per store,
/// then delegates to [`export_stores!`]. Later definitions may reference
/// earlier stores through their accessors (`count()` below).
///
/// Custom value types (`User` here) MUST derive `tsify::Tsify` (with the
/// `js` feature) — the generated `.d.ts` references the type by name, and
/// without a Tsify-emitted interface the TypeScript build breaks.
///
/// ```ignore
/// nanostores_wasm::define_stores! {
///     pub fn stores() -> StoreHandles {
///         atom count: i64 = 0;
///         map user: User = User::default();
///         readable doubled: i64 = computed((count().clone(),), |v| v * 2);
///     }
/// }
///
/// #[wasm_bindgen]
/// pub fn increment() {
///     count().set(count().get() + 1);
/// }
/// ```
#[macro_export]
macro_rules! define_stores {
    (
        $(#[$meta:meta])*
        pub fn $stores_fn:ident() -> $handles_ty:ident {
            $($kind:ident $name:ident : $value_ty:ident = $init:expr;)*
        }
    ) => {
        $(
            $crate::__nanostores_wasm_define_store!($kind, $name, $value_ty, $init);
        )*

        $crate::export_stores! {
            $(#[$meta])*
            pub fn $stores_fn() -> $handles_ty {
                $($kind $name : $value_ty = $name();)*
            }
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nanostores_wasm_define_store {
    (atom, $name:ident, $value_ty:ident, $init:expr) => {
        pub fn $name() -> &'static ::nanostores::Atom<$value_ty> {
            static STORE: ::std::sync::LazyLock<::nanostores::Atom<$value_ty>> =
                ::std::sync::LazyLock::new(|| ::nanostores::atom($init));
            ::std::sync::LazyLock::force(&STORE)
        }
    };
    (map, $name:ident, $value_ty:ident, $init:expr) => {
        pub fn $name() -> &'static ::nanostores::MapStore<$value_ty> {
            static STORE: ::std::sync::LazyLock<::nanostores::MapStore<$value_ty>> =
                ::std::sync::LazyLock::new(|| ::nanostores::map($init));
            ::std::sync::LazyLock::force(&STORE)
        }
    };
    (readable, $name:ident, $value_ty:ident, $init:expr) => {
        pub fn $name() -> &'static ::nanostores::AnyStore<$value_ty> {
            static STORE: ::std::sync::LazyLock<::nanostores::AnyStore<$value_ty>> =
                ::std::sync::LazyLock::new(|| ::nanostores::AnyStore::new($init));
            ::std::sync::LazyLock::force(&STORE)
        }
    };
}

/// Export existing stores to JS. Prefer [`define_stores!`] unless the store
/// statics already exist. Custom value types MUST derive `tsify::Tsify`
/// (`js` feature) — the emitted `.d.ts` references them by name.
#[macro_export]
macro_rules! export_stores {
    (
        $(#[$meta:meta])*
        pub fn $stores_fn:ident() -> $handles_ty:ident {
            $($kind:ident $name:ident : $value_ty:ident = $store:expr;)*
        }
    ) => {
        $(#[$meta])*
        #[wasm_bindgen::prelude::wasm_bindgen(skip_typescript)]
        pub fn $stores_fn() -> wasm_bindgen::JsValue {
            let builder = $crate::export();
            $(
                let builder = $crate::__nanostores_wasm_add_store!(
                    builder,
                    $kind,
                    stringify!($name),
                    $store
                );
            )*
            builder.build()
        }

        #[wasm_bindgen::prelude::wasm_bindgen(js_name = storeKinds, skip_typescript)]
        pub fn store_kinds() -> wasm_bindgen::JsValue {
            $crate::__nanostores_wasm_store_kinds(&[
                $(
                    (stringify!($name), $crate::__nanostores_wasm_kind_name!($kind)),
                )*
            ])
        }

        #[wasm_bindgen::prelude::wasm_bindgen(typescript_custom_section)]
        const __NANOSTORES_WASM_STORE_TYPES: &'static str = concat!(
            "import type { MapStore as NanoMapStore, ReadableAtom as NanoReadableAtom, WritableAtom as NanoWritableAtom } from 'nanostores';\n",
            "export interface ", stringify!($handles_ty), " {\n",
            $(
                "  ", stringify!($name), ": ",
                $crate::__nanostores_wasm_handle_ts_type!($kind, $value_ty),
                ";\n",
            )*
            "}\n",
            "export interface StoreKinds {\n",
            $(
                "  ", stringify!($name), ": \"",
                $crate::__nanostores_wasm_kind_name!($kind),
                "\";\n",
            )*
            "}\n",
            "export interface ProjectedStoreHandles {\n",
            $(
                "  ", stringify!($name), ": ",
                $crate::__nanostores_wasm_projected_ts_type!($kind, $value_ty),
                ";\n",
            )*
            "}\n",
            "export function ", stringify!($stores_fn), "(): ", stringify!($handles_ty), ";\n",
            "export function storeKinds(): StoreKinds;\n",
        );
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nanostores_wasm_add_store {
    ($builder:expr, atom, $name:expr, $store:expr) => {
        $builder.atom($name, $store)
    };
    ($builder:expr, map, $name:expr, $store:expr) => {
        $builder.map($name, $store)
    };
    ($builder:expr, readable, $name:expr, $store:expr) => {
        $builder.readable($name, $store)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nanostores_wasm_kind_name {
    (atom) => {
        "atom"
    };
    (map) => {
        "map"
    };
    (readable) => {
        "readable"
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nanostores_wasm_handle_ts_type {
    (atom, $value_ty:ident) => {
        concat!(
            "AtomHandle & { get(): ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            "; set(value: ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            "): void; subscribe(callback: (value: ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            ") => void): SubscriptionHandle }"
        )
    };
    (map, $value_ty:ident) => {
        concat!(
            "MapHandle & { get(): ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            "; set(value: ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            "): void; setKey<K extends keyof ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            " & string>(key: K, value: ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            "[K]): void; subscribe(callback: (value: ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            ", changedKey?: keyof ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            " & string) => void): SubscriptionHandle }"
        )
    };
    (readable, $value_ty:ident) => {
        concat!(
            "AtomHandle & { get(): ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            "; subscribe(callback: (value: ",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            ") => void): SubscriptionHandle }"
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nanostores_wasm_projected_ts_type {
    (atom, $value_ty:ident) => {
        concat!(
            "NanoWritableAtom<",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            ">"
        )
    };
    (map, $value_ty:ident) => {
        concat!(
            "NanoMapStore<",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            ">"
        )
    };
    (readable, $value_ty:ident) => {
        concat!(
            "NanoReadableAtom<",
            $crate::__nanostores_wasm_ts_type!($value_ty),
            ">"
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __nanostores_wasm_ts_type {
    (i8) => {
        "number"
    };
    (i16) => {
        "number"
    };
    (i32) => {
        "number"
    };
    (i64) => {
        "number"
    };
    (isize) => {
        "number"
    };
    (u8) => {
        "number"
    };
    (u16) => {
        "number"
    };
    (u32) => {
        "number"
    };
    (u64) => {
        "number"
    };
    (usize) => {
        "number"
    };
    (f32) => {
        "number"
    };
    (f64) => {
        "number"
    };
    (bool) => {
        "boolean"
    };
    (String) => {
        "string"
    };
    ($value_ty:ident) => {
        stringify!($value_ty)
    };
}
