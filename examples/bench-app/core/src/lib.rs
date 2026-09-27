#![cfg(target_arch = "wasm32")]

//! Benchmark wasm core: one atom per payload shape plus a `batched` store,
//! and `tick_*` functions that mutate state from the Rust side so that only
//! the notify direction crosses the JS/wasm boundary.

use nanostores::{Atom, Collection, NanoMap, batched, collection};
use serde::{Deserialize, Serialize};
use tsify::Tsify;
use wasm_bindgen::prelude::*;

const TEXT_A: &str = "nanostores-rs bench payload a a a a a a a "; // 41 bytes
const TEXT_B: &str = "nanostores-rs bench payload b b b b b b b ";

#[derive(Tsify, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: u32,
    pub name: String,
    pub score: f64,
    pub active: bool,
    pub tag: String,
}

#[derive(NanoMap, Tsify, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsPayload {
    pub rows: Vec<Row>,
}

fn row(id: u32, variant: u32) -> Row {
    Row {
        id: id + variant * 1_000_000,
        name: format!("row-{variant}-{id:05}"),
        score: id as f64 * 0.5 + variant as f64,
        active: (id + variant).is_multiple_of(2),
        tag: if variant == 0 { "bench-a" } else { "bench-b" }.to_owned(),
    }
}

fn rows(count: u32, variant: u32) -> RowsPayload {
    RowsPayload {
        rows: (0..count).map(|id| row(id, variant)).collect(),
    }
}

fn bench_collection() -> nanostores::CollectionStore<u32, Row> {
    let mut items = std::collections::HashMap::with_capacity(1250);
    let mut order = Vec::with_capacity(1250);
    for id in 0..1250_u32 {
        items.insert(id, row(id, 0));
        order.push(id);
    }
    collection(Collection::new(items, order))
}

nanostores_wasm::define_stores! {
    pub fn stores() -> StoreHandles {
        atom scalar: i32 = 0;
        atom text: String = TEXT_A.to_owned();
        atom small: Row = row(0, 0);
        atom medium: RowsPayload = rows(120, 0);
        atom large: RowsPayload = rows(1250, 0);
        readable batched_scalar: i32 = batched((scalar().clone(),), |value| value);
        collection list: (u32, Row) = bench_collection();
    }
}

/// Rust-side mutation of `scalar`: no input crossing, only the notification
/// crosses back to JS subscribers.
#[wasm_bindgen]
pub fn tick_scalar() {
    let next = if scalar().get() == 0 { 1 } else { 0 };
    scalar().set(next);
}

#[wasm_bindgen]
pub fn tick_text() {
    let next = if text().get() == TEXT_A { TEXT_B } else { TEXT_A };
    text().set(next.to_owned());
}

#[wasm_bindgen]
pub fn tick_small() {
    let mut value = small().get();
    value.id = value.id.wrapping_add(1);
    value.active = !value.active;
    small().set(value);
}

#[wasm_bindgen]
pub fn tick_medium() {
    flip_first_row(medium());
}

#[wasm_bindgen]
pub fn tick_large() {
    flip_first_row(large());
}

fn flip_first_row(store: &Atom<RowsPayload>) {
    let mut value = store.get();
    let first = &mut value.rows[0];
    first.id = first.id.wrapping_add(1);
    first.active = !first.active;
    store.set(value);
}

/// Edit ONE row of the 1250-row collection: only that row's subscribers are
/// notified, and only that row crosses the boundary.
#[wasm_bindgen]
pub fn tick_list_row() {
    list().update_item(0, |row| {
        row.id = row.id.wrapping_add(1);
        row.active = !row.active;
        true
    });
}

/// Rotate the collection's order: only the key array crosses.
#[wasm_bindgen]
pub fn tick_list_order() {
    let mut order = list().order();
    if let Some(first) = order.first().cloned() {
        order.remove(0);
        order.push(first);
        list().set_order(order);
    }
}
