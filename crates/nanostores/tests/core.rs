use nanostores::{
    AnyStore, NanoMap, Scheduler, atom, batched, computed, flush, map, on_mount, on_notify, on_set,
    on_start, on_stop, set_scheduler,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

#[derive(NanoMap, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    name: String,
    age: u32,
    display_name: Option<String>,
}

struct InlineScheduler;

impl Scheduler for InlineScheduler {
    fn schedule(&self, flush: Box<dyn FnOnce() + Send>) {
        flush();
    }
}

static SCHEDULER_TEST_LOCK: Mutex<()> = Mutex::new(());

struct SchedulerTestGuard {
    _lock: MutexGuard<'static, ()>,
}

impl Drop for SchedulerTestGuard {
    fn drop(&mut self) {
        flush();
        set_scheduler(InlineScheduler);
        flush();
    }
}

fn scheduler_test_guard() -> SchedulerTestGuard {
    let lock = SCHEDULER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    flush();
    set_scheduler(InlineScheduler);
    SchedulerTestGuard { _lock: lock }
}

#[test]
fn atom_subscribe_listen_skip_and_drop() {
    let _scheduler = scheduler_test_guard();
    let count = atom(1);
    let events = Arc::new(Mutex::new(Vec::new()));

    let subscription = count.subscribe({
        let events = Arc::clone(&events);
        move |value, key| {
            events
                .lock()
                .unwrap()
                .push((*value, key.map(str::to_owned)))
        }
    });

    count.set(2);
    count.set(2);
    assert_eq!(*events.lock().unwrap(), vec![(1, None), (2, None)],);

    drop(subscription);
    count.set(3);
    assert_eq!(*events.lock().unwrap(), vec![(1, None), (2, None)],);

    let future_events = Arc::new(Mutex::new(Vec::new()));
    let _listen = count.listen({
        let future_events = Arc::clone(&future_events);
        move |value, _| future_events.lock().unwrap().push(*value)
    });

    assert!(future_events.lock().unwrap().is_empty());
    count.set(4);
    assert_eq!(*future_events.lock().unwrap(), vec![4]);
}

#[test]
fn atom_listener_can_reenter_set() {
    let _scheduler = scheduler_test_guard();
    let count = atom(0);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let _subscription = count.listen({
        let count = count.clone();
        let seen = Arc::clone(&seen);
        move |value, _| {
            seen.lock().unwrap().push(*value);
            if *value == 1 {
                count.set(2);
            }
        }
    });

    count.set(1);

    assert_eq!(count.get(), 2);
    assert_eq!(*seen.lock().unwrap(), vec![1, 2]);
}

#[test]
fn map_set_key_listen_keys_and_typed_setters() {
    let _scheduler = scheduler_test_guard();
    let user = map(User {
        name: "Ada".to_owned(),
        age: 36,
        display_name: None,
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let _subscription = user.listen_keys(&["name", "displayName"], {
        let events = Arc::clone(&events);
        move |value, key| {
            events.lock().unwrap().push((
                value.name.clone(),
                value.age,
                value.display_name.clone(),
                key.map(str::to_owned),
            ));
        }
    });

    user.set_age(37);
    assert!(events.lock().unwrap().is_empty());

    user.set_name("Grace".to_owned());
    user.set_key("displayName", serde_json::json!("G.H."))
        .unwrap();

    assert_eq!(
        *events.lock().unwrap(),
        vec![
            ("Grace".to_owned(), 37, None, Some("name".to_owned())),
            (
                "Grace".to_owned(),
                37,
                Some("G.H.".to_owned()),
                Some("displayName".to_owned())
            ),
        ],
    );

    assert_eq!(User::KEYS, &["name", "age", "displayName"]);
    assert_eq!(
        user.get()
            .get_field("displayName", serde_json::value::Serializer)
            .unwrap(),
        serde_json::json!("G.H."),
    );
    assert!(user.set_key("missing", serde_json::json!(1)).is_err());
}

#[test]
fn map_update_notifies_all_key_listeners() {
    let _scheduler = scheduler_test_guard();
    let user = map(User {
        name: "Ada".to_owned(),
        age: 36,
        display_name: None,
    });
    let called = Arc::new(AtomicUsize::new(0));
    let _subscription = user.listen_keys(&["name"], {
        let called = Arc::clone(&called);
        move |_, key| {
            assert_eq!(key, None);
            called.fetch_add(1, Ordering::SeqCst);
        }
    });

    user.update(|user| user.age += 1);

    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[test]
fn computed_is_lazy_unmounted_and_live_while_mounted() {
    let _scheduler = scheduler_test_guard();
    let a = atom(1);
    let b = atom(2);
    let sum = computed((a.clone(), b.clone()), |a, b| a + b);

    assert_eq!(sum.get(), 3);
    a.set(2);
    assert_eq!(sum.get(), 4);

    let events = Arc::new(Mutex::new(Vec::new()));
    let subscription = sum.subscribe({
        let events = Arc::clone(&events);
        move |value, _| events.lock().unwrap().push(*value)
    });

    b.set(5);
    b.set(5);

    assert_eq!(*events.lock().unwrap(), vec![4, 7]);
    drop(subscription);

    a.set(10);
    assert_eq!(*events.lock().unwrap(), vec![4, 7]);
    assert_eq!(sum.get(), 15);
}

#[test]
fn batched_collapses_changes_until_flush() {
    let _scheduler = scheduler_test_guard();
    struct ManualScheduler;
    impl Scheduler for ManualScheduler {
        fn schedule(&self, _flush: Box<dyn FnOnce() + Send>) {}
    }

    set_scheduler(ManualScheduler);

    let count = atom(1);
    let recomputes = Arc::new(AtomicUsize::new(0));
    let doubled = batched((count.clone(),), {
        let recomputes = Arc::clone(&recomputes);
        move |count| {
            recomputes.fetch_add(1, Ordering::SeqCst);
            count * 2
        }
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let _subscription = doubled.subscribe({
        let events = Arc::clone(&events);
        move |value, _| events.lock().unwrap().push(*value)
    });

    count.set(2);
    count.set(3);
    assert_eq!(*events.lock().unwrap(), vec![2]);

    flush();
    assert_eq!(*events.lock().unwrap(), vec![2, 6]);
    assert_eq!(recomputes.load(Ordering::SeqCst), 3);
}

#[test]
fn computed_unmount_drops_dependency_subscriptions() {
    let _scheduler = scheduler_test_guard();
    let source = atom(1);
    let starts = Arc::new(AtomicUsize::new(0));
    let stops = Arc::new(AtomicUsize::new(0));
    let _source_start = on_start(&source, {
        let starts = Arc::clone(&starts);
        move || {
            starts.fetch_add(1, Ordering::SeqCst);
        }
    });
    let _source_stop = on_stop(&source, {
        let stops = Arc::clone(&stops);
        move || {
            stops.fetch_add(1, Ordering::SeqCst);
        }
    });
    let doubled = computed((source.clone(),), |value| value * 2);

    let subscription = doubled.listen(|_, _| {});
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(stops.load(Ordering::SeqCst), 0);

    drop(subscription);

    assert_eq!(stops.load(Ordering::SeqCst), 1);
}

#[test]
fn computed_diamond_dependency_notifies_only_consistent_value() {
    let _scheduler = scheduler_test_guard();
    let source = atom(1);
    let left = computed((source.clone(),), |value| value * 2);
    let right = computed((source.clone(),), |value| value * 3);
    let total = computed((left, right), |left, right| left + right);
    let events = Arc::new(Mutex::new(Vec::new()));
    let _subscription = total.subscribe({
        let events = Arc::clone(&events);
        move |value, _| events.lock().unwrap().push(*value)
    });

    source.set(2);

    assert_eq!(*events.lock().unwrap(), vec![5, 10]);
}

#[test]
fn lifecycle_hooks_start_stop_set_and_notify() {
    let _scheduler = scheduler_test_guard();
    let count = atom(1);
    let lifecycle = Arc::new(Mutex::new(Vec::new()));
    let _start = on_start(&count, {
        let lifecycle = Arc::clone(&lifecycle);
        move || lifecycle.lock().unwrap().push("start")
    });
    let _stop = on_stop(&count, {
        let lifecycle = Arc::clone(&lifecycle);
        move || lifecycle.lock().unwrap().push("stop")
    });
    let _mount = on_mount(&count, {
        let lifecycle = Arc::clone(&lifecycle);
        move || {
            lifecycle.lock().unwrap().push("mount");
            let lifecycle = Arc::clone(&lifecycle);
            move || lifecycle.lock().unwrap().push("cleanup")
        }
    });

    let subscription = count.subscribe(|_, _| {});
    drop(subscription);
    assert_eq!(
        *lifecycle.lock().unwrap(),
        vec!["start", "mount", "cleanup", "stop"],
    );

    let _set = on_set(&count, |ctx| {
        if *ctx.value() == 2 {
            ctx.abort();
        } else {
            *ctx.value_mut() *= 10;
        }
    });
    count.set(2);
    assert_eq!(count.get(), 1);
    count.set(3);
    assert_eq!(count.get(), 30);

    let notifications = Arc::new(Mutex::new(Vec::new()));
    let _notify = on_notify(&count, |ctx| {
        if *ctx.value() == 40 {
            ctx.abort();
        }
    });
    let _listener = count.listen({
        let notifications = Arc::clone(&notifications);
        move |value, _| notifications.lock().unwrap().push(*value)
    });

    count.set(4);
    count.set(5);

    assert_eq!(count.get(), 50);
    assert_eq!(*notifications.lock().unwrap(), vec![50]);
}

#[test]
fn any_store_erases_computed_type_and_stays_reactive() {
    let _scheduler = scheduler_test_guard();
    let count = atom(2);
    let doubled: AnyStore<i64> = AnyStore::new(computed((count.clone(),), |v| v * 2));

    assert_eq!(doubled.get(), 4);

    let events = Arc::new(Mutex::new(Vec::new()));
    let _subscription = doubled.subscribe({
        let events = Arc::clone(&events);
        move |value, _| events.lock().unwrap().push(*value)
    });
    count.set(5);

    assert_eq!(*events.lock().unwrap(), vec![4, 10]);
}

#[test]
fn concurrent_sets_and_cross_store_listener_do_not_deadlock() {
    let _scheduler = scheduler_test_guard();
    let source = atom(0);
    let target = atom(0);
    let _subscription = source.listen({
        let target = target.clone();
        move |value, _| target.set(*value)
    });

    let handles = (1..=8)
        .map(|value| {
            let source = source.clone();
            thread::spawn(move || source.set(value))
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }

    source.set(100);
    assert_eq!(target.get(), 100);
}
