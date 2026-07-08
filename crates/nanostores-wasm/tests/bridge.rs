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
