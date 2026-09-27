#![cfg(target_arch = "wasm32")]

use nanostores::{NanoMap, atom, computed, map, on_set};
use nanostores_wasm::{AtomHandle, MapHandle};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[derive(NanoMap, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    name: String,
    age: u32,
    display_name: Option<String>,
}

#[wasm_bindgen_test]
fn atom_handle_round_trip_and_subscription() {
    let count = atom(1_i32);
    let handle = AtomHandle::from_atom(&count);

    assert_eq!(from_js::<i32>(handle.get().unwrap()), 1);

    let seen = Rc::new(RefCell::new(Vec::new()));
    let callback = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new({
        let seen = Rc::clone(&seen);
        move |value| seen.borrow_mut().push(from_js::<i32>(value))
    });
    let mut subscription = handle.subscribe(
        callback
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone(),
    );

    handle.set(to_js(&2_i32)).unwrap();
    subscription.unsubscribe();
    handle.set(to_js(&3_i32)).unwrap();

    assert_eq!(*seen.borrow(), vec![2]);
    assert_eq!(count.get(), 3);
}

#[wasm_bindgen_test]
fn map_handle_set_key_crosses_only_changed_key() {
    let user = map(User {
        name: "Ada".to_owned(),
        age: 36,
        display_name: None,
    });
    let handle = MapHandle::from_map(&user);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let callback = Closure::<dyn FnMut(wasm_bindgen::JsValue, wasm_bindgen::JsValue)>::new({
        let seen = Rc::clone(&seen);
        move |value: wasm_bindgen::JsValue, key: wasm_bindgen::JsValue| {
            let user = from_js::<User>(value);
            seen.borrow_mut().push((user.display_name, key.as_string()));
        }
    });
    let _subscription = handle.subscribe(
        callback
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone(),
    );

    handle
        .set_key("displayName".to_owned(), to_js(&"A.L.".to_owned()))
        .unwrap();

    assert_eq!(user.get().display_name, Some("A.L.".to_owned()));
    assert_eq!(
        *seen.borrow(),
        vec![(Some("A.L.".to_owned()), Some("displayName".to_owned()))],
    );
}

#[wasm_bindgen_test]
fn atom_handle_reports_deserialize_errors_without_mutating() {
    let count = atom(1_i32);
    let handle = AtomHandle::from_atom(&count);

    let result = handle.set(wasm_bindgen::JsValue::from_str("not a number"));

    assert!(result.is_err());
    assert_eq!(count.get(), 1);
}

#[wasm_bindgen_test]
fn map_handle_rejected_set_key_leaves_value_unchanged() {
    let user = map(User {
        name: "Ada".to_owned(),
        age: 36,
        display_name: None,
    });
    let _guard = on_set(&user, |ctx| {
        if ctx.changed_key() == Some("age") {
            ctx.abort();
        }
    });
    let handle = MapHandle::from_map(&user);

    handle.set_key("age".to_owned(), to_js(&37_u32)).unwrap();

    assert_eq!(user.get().age, 36);
}

#[wasm_bindgen_test]
fn readable_atom_handle_rejects_writes() {
    let count = atom(2_i32);
    let doubled = computed((count.clone(),), |count| count * 2);
    let handle = AtomHandle::from_readable(&doubled);

    assert_eq!(from_js::<i32>(handle.get().unwrap()), 4);
    assert!(handle.set(to_js(&10_i32)).is_err());
    assert_eq!(doubled.get(), 4);
}

#[wasm_bindgen_test]
fn atom_echo_reuses_the_js_object_passed_to_set() {
    let user = atom(User {
        name: "Ada".to_owned(),
        age: 36,
        display_name: None,
    });
    let handle = AtomHandle::from_atom(&user);

    let seen = Rc::new(RefCell::new(Vec::new()));
    let callback = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new({
        let seen = Rc::clone(&seen);
        move |value| seen.borrow_mut().push(value)
    });
    let _subscription = handle.subscribe(
        callback
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone(),
    );

    // Equal content, distinct objects: the notification must carry the exact
    // object handed to set (echo reuse), not a re-serialized copy.
    let first = to_js(&User {
        name: "Ada".to_owned(),
        age: 37,
        display_name: None,
    });
    handle.set(first.clone()).unwrap();

    // Content-equal write: skipped by Rust-side change detection, no
    // notification at all.
    let second = to_js(&User {
        name: "Ada".to_owned(),
        age: 37,
        display_name: None,
    });
    handle.set(second.clone()).unwrap();

    // A Rust-side write of different content still crosses with a fresh
    // serialization and must not reuse the echoed object.
    user.set(User {
        name: "Ada".to_owned(),
        age: 38,
        display_name: None,
    });

    let seen = seen.borrow();
    assert_eq!(seen.len(), 2);
    assert_eq!(from_js::<User>(seen[0].clone()).age, 37);
    assert!(js_sys::Object::is(&seen[0], &first));
    assert!(!js_sys::Object::is(&seen[1], &second));
    assert_eq!(from_js::<User>(seen[1].clone()).age, 38);
}

#[wasm_bindgen_test]
fn map_echo_reuses_the_js_object_passed_to_set_but_not_set_key() {
    let user = map(User {
        name: "Ada".to_owned(),
        age: 36,
        display_name: None,
    });
    let handle = MapHandle::from_map(&user);

    let seen = Rc::new(RefCell::new(Vec::new()));
    let callback = Closure::<dyn FnMut(wasm_bindgen::JsValue, wasm_bindgen::JsValue)>::new({
        let seen = Rc::clone(&seen);
        move |value, key| seen.borrow_mut().push((value, key))
    });
    let _subscription = handle.subscribe(
        callback
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone(),
    );

    let whole = to_js(&User {
        name: "Ada".to_owned(),
        age: 37,
        display_name: None,
    });
    handle.set(whole.clone()).unwrap();

    {
        let mut guard = seen.borrow_mut();
        assert_eq!(guard.len(), 1);
        let (value, key) = guard.remove(0);
        assert!(js_sys::Object::is(&value, &whole));
        assert!(key.is_undefined());
    }

    // setKey mutates one field in Rust; the notification re-serializes the
    // full map and must NOT reuse the object passed to the earlier set.
    handle
        .set_key("displayName".to_owned(), to_js(&"A.L.".to_owned()))
        .unwrap();

    let guard = seen.borrow();
    assert_eq!(guard.len(), 1);
    let (value, key) = (&guard[0].0, &guard[0].1);
    assert!(!js_sys::Object::is(value, &whole));
    assert_eq!(key.as_string().as_deref(), Some("displayName"));
    assert_eq!(
        from_js::<User>(value.clone()).display_name.as_deref(),
        Some("A.L.")
    );
}

fn to_js<T>(value: &T) -> wasm_bindgen::JsValue
where
    T: Serialize,
{
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    value.serialize(&serializer).unwrap()
}

fn from_js<T>(value: wasm_bindgen::JsValue) -> T
where
    T: for<'de> Deserialize<'de>,
{
    serde_wasm_bindgen::from_value(value).unwrap()
}
