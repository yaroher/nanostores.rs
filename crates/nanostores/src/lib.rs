//! Thread-safe Rust stores modeled after the JavaScript nanostores API.

mod any;
mod atom;
mod computed;
mod error;
mod lifecycle;
mod map;
mod scheduler;
mod store;
mod subscription;

pub use any::AnyStore;
pub use atom::{Atom, atom};
pub use computed::{Batched, Computed, batched, computed};
pub use error::KeyError;
pub use lifecycle::{Lifecycle, on_mount, on_notify, on_set, on_start, on_stop};
pub use map::{MapStore, NanoMap, map};
pub use scheduler::{Scheduler, flush, set_scheduler, set_scheduler_if_unset};
pub use store::{ChangeContext, Listener, NotifyContext, ReadableStore, SetContext, StoreLike};
pub use subscription::Subscription;

#[cfg(feature = "derive")]
pub use nanostores_macros::NanoMap;
