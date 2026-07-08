#![cfg(target_arch = "wasm32")]

use nanostores::{Atom, Computed, MapStore, NanoMap, atom, batched, computed, map};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tsify::Tsify;
use wasm_bindgen::prelude::*;

#[derive(NanoMap, Tsify, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub name: String,
    pub age: u32,
    pub display_name: Option<String>,
}

static COUNT: LazyLock<Atom<i64>> = LazyLock::new(|| atom(0));
static USER: LazyLock<MapStore<User>> = LazyLock::new(|| {
    map(User {
        name: "Ada".to_owned(),
        age: 36,
        display_name: Some("Ada Lovelace".to_owned()),
    })
});
type CountDeps = (Atom<i64>,);
type DoubledStore = Computed<CountDeps, fn(i64) -> i64, i64>;
type SummaryDeps = (Atom<i64>, MapStore<User>);
type SummaryStore = Computed<SummaryDeps, fn(i64, User) -> String, String>;

static DOUBLED: LazyLock<DoubledStore> =
    LazyLock::new(|| computed((LazyLock::force(&COUNT).clone(),), double));
static SUMMARY: LazyLock<SummaryStore> = LazyLock::new(|| {
    batched(
        (
            LazyLock::force(&COUNT).clone(),
            LazyLock::force(&USER).clone(),
        ),
        summary,
    )
});

nanostores_wasm::export_stores! {
    pub fn stores() -> StoreHandles {
        atom count: i64 = LazyLock::force(&COUNT);
        map user: User = LazyLock::force(&USER);
        readable doubled: i64 = LazyLock::force(&DOUBLED);
        readable summary: String = LazyLock::force(&SUMMARY);
    }
}

#[wasm_bindgen]
pub fn increment() {
    let count = LazyLock::force(&COUNT);
    count.set(count.get() + 1);
}

fn double(value: i64) -> i64 {
    value * 2
}

fn summary(count: i64, user: User) -> String {
    let name = user.display_name.as_deref().unwrap_or(&user.name);
    format!("{name} has {count} clicks")
}
