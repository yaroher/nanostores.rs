# nanostores.rs — design

**Date:** 2026-07-08
**Status:** approved for implementation planning

## 1. Goal

A Rust port of [nanostores](https://github.com/nanostores/nanostores) plus a
wasm bridge that projects Rust-owned reactive state into real JS nanostores
stores, preserving reactivity in both directions.

- **Source of truth lives in Rust.** The wasm module owns the state.
- **TS side sees ordinary nanostores stores.** `@nanostores/react`,
  `computed`, `listenKeys` and the rest of the JS ecosystem work on
  projections without knowing they are projections.
- **Writes are bidirectional.** `$store.set(v)` / `$store.setKey(k, v)` in TS
  forwards into Rust; Rust applies (and may transform/reject), the change
  notification flows back and updates the projection. TS code cannot tell a
  projection from a native atom.
- The core crate is a **standalone, thread-safe reactive library** usable in
  native Rust programs without wasm.

### Non-goals (v0.1)

- `deepMap` / `getPath` / `setPath` — v0.2.
- `effect`, `task` / `allTasks`, `keepMount`, `cleanStores`, `mapCreator` —
  later.
- Protobuf anywhere. Out of scope entirely (dropped by decision).
- SSR concerns.
- Persistence, router, i18n and other nanostores satellite packages.

## 2. Prior-art constraints (research summary)

Decisions below follow a July 2026 research pass:

- **tsify 0.5.6** is alive (tsify-next is dead, RUSTSEC-2025-0048). Use the
  `js` feature (serde-wasm-bindgen backend) and the `Ts<T>` wrapper style —
  the `into_wasm_abi`/`from_wasm_abi` container attributes are deprecated.
  Avoid `#[serde(flatten)]` in boundary types (broken with the js backend).
- **serde-wasm-bindgen 0.6** is the official conversion path. Configure
  `serialize_maps_as_objects(true)`; decide `null` vs `undefined` policy once:
  we use **`None` → `undefined`** (nanostores/JS convention for absent map
  keys) — do not enable `serialize_missing_as_null`.
- Every boundary crossing creates a **fresh JS object** — no `===` stability.
  Therefore change detection (`PartialEq` skip) happens **on the Rust side**;
  the JS projection applies whatever arrives.
- Mature wasm libs (automerge, loro) never re-serialize whole documents per
  update; they cross events/patches. We adopt the middle ground below.

## 3. Bridge granularity (decision: hybrid "C")

Granularity of boundary crossings = granularity of the nanostores API itself.
No diff engine.

- `atom` update → the whole value crosses (that is what `atom.set` means in
  JS too).
- `map` `set_key` → only `(key, value)` crosses; the TS projection calls
  `setKey(key, value)` on a real nanostores map, so JS `listenKeys`
  subscribers work natively.
- `map` whole-value `set` → whole object crosses once, projection calls
  `set`.

## 4. Workspace layout

Mirrors the sibling project `slozhn` (workspace + examples-as-members + CI).

```
Cargo.toml                      # workspace, resolver = "3", edition 2024
crates/
  nanostores/                   # core reactive library (pure Rust, no wasm deps)
  nanostores-macros/            # #[derive(NanoMap)] proc-macro
  nanostores-wasm/              # wasm-bindgen bridge (wasm32-only code paths)
packages/
  nanostores-wasm/              # TS glue tests + wrapper generation tooling
examples/
  browser-app/
    core/                       # wasm-pack crate: demo state defined in Rust
    ui/                         # vite + TS app consuming projections
.github/workflows/ci.yml
```

Crate names on crates.io must be checked before publishing (`nanostores` may
be taken); the workspace uses these names internally regardless, publishing
names are a release-time concern. npm package placeholder name:
`nanostores-wasm`.

## 5. `crates/nanostores` — core

### 5.1 Threading model

**Thread-safe from day one** (decision). All stores are `Send + Sync`,
cheaply cloneable handles:

```rust
pub struct Atom<T>(Arc<Inner<T>>);   // Clone = handle clone
```

- Value behind `RwLock<T>`; listener registry behind a separate
  `Mutex<Listeners<T>>`.
- **Locking rule (deadlock safety):** listeners are never invoked while any
  store lock is held. `set` sequence: take value lock → equality check →
  write → drop lock → snapshot listener `Arc`s under the registry lock →
  drop lock → invoke snapshot. Reentrant `set` from inside a listener is
  therefore legal (same as JS nanostores).
- Notification delivers the **new value by reference** (`&T`) plus
  `changed_key: Option<&str>` for maps.

### 5.2 Types and API surface (v0.1)

Semantics track nanostores 1.4.x. Rust naming is snake_case.

```rust
// creation
pub fn atom<T: Clone + PartialEq + Send + Sync + 'static>(init: T) -> Atom<T>;
pub fn map<T: NanoMap + Clone + PartialEq + Send + Sync + 'static>(init: T) -> MapStore<T>;
pub fn computed<T, F>(deps: (/* tuple of stores */), f: F) -> Computed<T>;
pub fn batched<T, F>(deps: (/* tuple of stores */), f: F) -> Computed<T>; // deferred via Scheduler

// every store (trait ReadableStore<T>)
fn get(&self) -> T;                                   // clone of current value
fn subscribe(&self, f: impl Listener<T>) -> Subscription; // fires immediately + on change
fn listen(&self, f: impl Listener<T>) -> Subscription;    // future changes only

// writable stores (Atom, MapStore)
fn set(&self, value: T);                              // PartialEq skip if unchanged

// MapStore<T: NanoMap>
fn set_key<'de, D: serde::Deserializer<'de>>(&self, key: &str, value: D) -> Result<(), KeyError>;
fn update(&self, f: impl FnOnce(&mut T));             // notifies with changed_key = None (= all keys)
fn listen_keys(&self, keys: &[&str], f: impl Listener<T>) -> Subscription;
// plus typed per-field setters generated by #[derive(NanoMap)], see 5.5
```

- `Subscription` is RAII: `Drop` unsubscribes; `.detach()` leaks it
  intentionally (keeps the listener forever). This replaces the JS
  `unbind()` closure.
- `Listener<T>` ≈ `Fn(&T, Option<&str>) + Send + Sync + 'static`.
- `Atom::set` skips notify when `new == old` (mirrors `Object.is` skip in
  JS). `MapStore::set_key` compares the single field.

### 5.3 computed / batched

- `computed((a, b), |av, bv| ...)`: eager while mounted (recomputes on any
  dep notification, then notifies own listeners if value changed), lazy
  `get()` when unmounted (recompute on demand). Dep subscriptions are
  created on first own listener (`on_start`) and dropped on last
  (`on_stop`) — this is exactly nanostores mount semantics and prevents
  leaks.
- Dependency tuples are implemented for arities 1..=8 via macro (start with
  a handful; the pattern is mechanical).
- `batched` = same as computed but recompute is **deferred to the scheduler
  tick**; multiple dep changes in one tick collapse into one recompute.

**Scheduler** (needed because pure Rust has no microtask queue):

```rust
pub trait Scheduler: Send + Sync + 'static {
    fn schedule(&self, flush: Box<dyn FnOnce() + Send>);
}
pub fn set_scheduler(s: impl Scheduler);  // global, set once
pub fn flush();                           // drain pending batched recomputes now
```

- Default scheduler (native): **immediate** — `batched` degenerates to
  `computed`. Documented.
- Tests use a manual scheduler + `flush()` for determinism.
- The wasm glue installs a `queueMicrotask` scheduler at init (see §6.4),
  restoring exact JS `batched` semantics in the browser.

### 5.4 Lifecycle hooks

```rust
pub fn on_start(store, f) -> Subscription;   // first listener attached
pub fn on_stop(store, f) -> Subscription;    // last listener removed
pub fn on_mount(store, f) -> Subscription;   // = on_start + returned cleanup runs on stop
pub fn on_set(store, f) -> Subscription;     // before write; ctx.abort() cancels
pub fn on_notify(store, f) -> Subscription;  // before listeners; ctx.abort() cancels
```

- `on_mount` in JS delays unmount by `STORE_UNMOUNT_DELAY` (1 s) to survive
  re-renders. The Rust core has **no unmount delay** (no portable timer);
  divergence documented. The TS projection layer gets the delay for free
  because projections are real nanostores stores with native mount logic.
- `on_set` / `on_notify` receive a context struct: new value (mutable
  access for transforms — matches JS `changed`/`newValue` mutation ability),
  `changed_key`, and `abort()`.
- Hook chains run under the same "no locks held during callbacks" rule.

### 5.5 `#[derive(NanoMap)]` (crates/nanostores-macros)

Rust structs have no string-keyed field access, so the derive generates it:

```rust
#[derive(NanoMap, Clone, PartialEq, Serialize, Deserialize)]
struct User { name: String, age: u32 }
```

generates an impl of:

```rust
pub trait NanoMap: Sized {
    const KEYS: &'static [&'static str];
    /// Deserialize `value` into the field named `key`.
    fn set_field<'de, D: serde::Deserializer<'de>>(
        &mut self, key: &str, value: D,
    ) -> Result<(), KeyError>;
    /// Serialize a single field (used by the bridge for set_key echo).
    fn get_field<S: serde::Serializer>(
        &self, key: &str, ser: S,
    ) -> Result<S::Ok, KeyError>;
}
```

plus typed per-field setters on `MapStore<User>` (via a generated extension
trait): `store.set_name("x")`, `store.set_age(3)` — each notifies with the
right `changed_key`. Field names cross the boundary as **camelCase** if the
struct has `#[serde(rename_all = "camelCase")]`; the derive reads serde
rename attributes so Rust and TS agree on key strings.

Core depends on `serde` (mandatory); it does NOT depend on `serde_json`,
`wasm-bindgen`, or anything platform-specific.

## 6. `crates/nanostores-wasm` — bridge

`#![cfg(target_arch = "wasm32")]` code paths; crate-type `cdylib` + `rlib`.

### 6.1 Boundary types

- Values cross via **serde-wasm-bindgen** (`js` conversion, no JSON
  strings). Serializer config: `serialize_maps_as_objects(true)`; do NOT
  use `json_compatible()` (it enables `missing_as_null`, we want `None` →
  `undefined`).
- **tsify 0.5.6** (`js` feature) on user state types gives `.d.ts`
  interfaces; bridge API functions use `Ts<T>` wrappers. Users deriving
  `Tsify` on their state structs get a fully typed TS boundary; without it
  the projection value type is `any`.

### 6.2 Projection handles

The bridge exposes generic wasm classes the TS glue consumes:

```rust
#[wasm_bindgen]
pub struct AtomHandle { /* type-erased Arc<dyn ErasedAtom> */ }

#[wasm_bindgen]
impl AtomHandle {
    pub fn get(&self) -> JsValue;                       // serialize current
    pub fn set(&self, v: JsValue) -> Result<(), JsValue>; // deserialize + core set
    /// cb(newValue: JsValue) — called on every Rust-side change.
    pub fn subscribe(&self, cb: js_sys::Function) -> SubscriptionHandle;
}

#[wasm_bindgen]
pub struct MapHandle { ... }  // + set_key(key: String, v: JsValue),
                              //   subscribe cb(value, changedKey?: string)
```

Rust application code (the wasm "core" crate of an app) builds handles from
its typed stores:

```rust
static COUNT: LazyLock<Atom<i64>> = LazyLock::new(|| atom(0));
static USER: LazyLock<MapStore<User>> = LazyLock::new(|| map(User::default()));

#[wasm_bindgen]
pub fn stores() -> JsValue {
    bridge::export()          // builder API
        .atom("count", &*COUNT)
        .map("user", &*USER)
        .build()              // JS object: { count: AtomHandle, user: MapHandle }
}
```

Type erasure: `export().atom::<T>()` wraps the typed store in a
`dyn ErasedStore` whose vtable does the serde conversion; handles stay
non-generic so wasm-bindgen can export them.

### 6.3 JS callbacks vs `Send + Sync`

Core listeners require `Send + Sync`; `js_sys::Function` is neither. wasm32
is single-threaded, so the bridge wraps callbacks in
**`send_wrapper::SendWrapper`** (standard practice; panics only if actually
crossed to another thread, which cannot happen on wasm32-unknown-unknown).
This is confined to the bridge crate; the core stays honest.

### 6.4 Echo-guard & synchronous round trip

Everything is synchronous, so the write path is a single call stack:

```
TS $count.set(5)
  → handle.set(JsValue)            [glue forwards instead of local set]
    → Rust atom.set(5)             [may transform via on_set, or abort]
      → core listeners fire
        → bridge listener serializes → JS cb
          → glue applies value to the projection via its ORIGINAL set
            → nanostores subscribers fire (React etc.)
```

- The glue never calls the original `set` except from the wasm callback, so
  there is no echo loop and no double-fire by construction.
- If Rust `on_set` aborts, no notification comes back and the projection
  simply keeps its old value — TS `set` becomes a rejected write, exactly
  the desired "Rust may reject" semantics. `handle.set` also returns
  `Result` so the glue can surface deserialize errors to `console.error`.
- `batched` scheduler: `nanostores-wasm` calls
  `nanostores::set_scheduler(MicrotaskScheduler)` (via `queueMicrotask`) in
  its init function.

## 7. TS projection wrapper

The browser-facing layer is generated next to wasm-bindgen output. Rust
`export_stores!` emits the typed `.d.ts` contract (`StoreHandles`,
`StoreKinds`, `ProjectedStoreHandles`), then the wrapper generator writes
`*_stores.ts` beside the generated wasm module.

```ts
import { createStores } from "./pkg/browser_app_core_stores";

// wasm module init happens inside createStores (memoized) — no manual init.
const stores = await createStores();
```

Behavior of a projection:

- It is a **real** nanostores store created with `atom()` / `map()`.
- `onMount(projection, ...)`: on mount → `handle.subscribe(cb)` +
  initialize value from `handle.get()`; on unmount → drop the wasm
  subscription. Lazy: no JS listeners → no wasm traffic. JS-side
  `STORE_UNMOUNT_DELAY` applies natively.
- `set` / `setKey` are overridden to forward into the handle; the original
  implementations are kept privately and invoked only from the wasm
  callback (§6.4).
- The generated wrapper imports `nanostores` directly from the app, so the
  projection shares the app's nanostores instance without an app runtime
  dependency on a separate `nanostores-wasm` npm package.
- `packages/nanostores-wasm` still keeps the reusable projection tests and the
  generator script while the generation path is being hardened.

## 8. `examples/browser-app`

Same shape as slozhn's `examples/browser-app` (core + ui, ui's `pkg` output
gitignored):

- **core** (`wasm-pack build --target web`): defines
  `count: Atom<i64>`, `user: MapStore<User>` (2–3 fields), and
  `doubled: Computed<i64>` plus `summary: batched(...)` — both computed
  **in Rust** — exports them via `export_stores!`. Also exports one Rust
  "action" (`increment()`) to show Rust-initiated mutation reaching the UI.
- **ui** (vite + Preact + TS): imports generated stores from
  `ui/src/pkg/browser_app_core_stores.ts`, renders them with
  `@nanostores/preact` `useStore`, has inputs that call `$count.set`,
  `$user.setKey`, and a button for `core.increment()`. Demonstrates: TS write
  → Rust → projection; Rust write → projection; Rust computed reacting to TS
  write; batched computed projection; `listenKeys` on the user projection.
- README with run steps (`make build-example`, `npm run dev`).

## 9. CI (`.github/workflows/ci.yml`)

Copy slozhn's proven pipeline:

- **native**: `cargo test --workspace`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo doc --workspace --no-deps`.
  `Swatinem/rust-cache` with `save-if` master-only; `CARGO_PROFILE_DEV_DEBUG: 0`.
- **wasm**: `cargo build --target wasm32-unknown-unknown -p nanostores -p
  nanostores-wasm -p browser-app-core` + clippy for the same set.
- **browser**: `wasm-pack test --headless --chrome` for `nanostores-wasm`
  tests via `run-browser-tests.sh`; browser failures fail CI.
- **js**: `make test-js` rebuilds the example wasm package, generates the
  wrapper, typechecks, runs vitest, and does a production Vite build.
- `release.yml` reusing ci as a gate — later, when publishing.

## 10. Testing strategy

- **Core unit tests** (native, `cargo test`):
  - atom: set/get/subscribe-immediate/listen-future, PartialEq skip,
    Subscription drop unsubscribes, reentrant set from listener.
  - map: set_key single-key notify, listen_keys filtering, update()
    notifies all, derive-generated typed setters, serde-rename key names.
  - computed: laziness when unmounted, recompute on dep change when
    mounted, diamond dependency (a → b,c → d) delivers consistent value,
    unmount drops dep subscriptions.
  - batched: manual scheduler + flush() collapses N dep changes into one
    recompute.
  - lifecycle: on_start/on_stop ordering, on_set abort blocks write,
    on_set value transform, on_notify abort blocks listeners.
  - concurrency: N threads hammering set/subscribe/drop on one atom (no
    deadlock, no lost notifications for the final value); listener that
    calls set on another store.
- **Macro tests**: trybuild pass/fail cases for `#[derive(NanoMap)]`.
- **Bridge tests** (`wasm-bindgen-test`, headless chrome): JsValue round
  trip for atom + map, set_key crossing only the key, echo-guard (one
  notification per set), abort-from-on_set leaves projection untouched,
  microtask scheduler batches.
- **Glue tests** (vitest, node with wasm): projection behaves as a real
  nanostores store: `subscribe`, `listenKeys`, mount laziness
  (`handle.subscribe` called only when mounted).

## 11. Milestones

1. **M1 — core**: `crates/nanostores` atom + map + computed + batched +
   lifecycle + scheduler, `nanostores-macros` derive, full native test
   suite, CI native job green.
2. **M2 — bridge**: `crates/nanostores-wasm` handles + builder + SendWrapper
   glue + microtask scheduler, wasm-bindgen-tests, CI wasm/browser jobs.
3. **M3 — generated TS wrapper + example**: `packages/nanostores-wasm`
   projection tests/generator, `examples/browser-app`, vitest, README(s).

Each milestone lands as a reviewable unit with tests before the next
starts.

## 12. Resolved decisions (log)

| Question | Decision |
|---|---|
| Source of truth | Rust/wasm; TS nanostores = projections |
| Write path | Bidirectional `.set()`/`.setKey()`, Rust may reject/transform |
| Rust side shape | Full nanostores port (usable natively), bridge separate crate |
| Threading | Thread-safe (`Send + Sync`, Arc) from day one |
| v0.1 API | atom, map, computed, batched, subscribe/listen/listen_keys, on_start/on_stop/on_mount/on_set/on_notify |
| Deferred to v0.2+ | deepMap/setPath, effect, task, keepMount, mapCreator |
| Boundary format | serde-wasm-bindgen 0.6 (js backend) + tsify 0.5.6 `Ts<T>` for types |
| Protobuf | Dropped entirely |
| Bridge granularity | Hybrid: atom = whole value, map = (key, value) per setKey |
| `None` policy | `undefined` (not `null`) |
| Unmount delay | Only on the JS projection side (native nanostores); Rust core has none |
