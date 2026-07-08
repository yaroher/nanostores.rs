use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

pub trait Scheduler: Send + Sync + 'static {
    fn schedule(&self, flush: Box<dyn FnOnce() + Send>);
}

struct ImmediateScheduler;

impl Scheduler for ImmediateScheduler {
    fn schedule(&self, flush: Box<dyn FnOnce() + Send>) {
        flush();
    }
}

static SCHEDULER: LazyLock<RwLock<Arc<dyn Scheduler>>> =
    LazyLock::new(|| RwLock::new(Arc::new(ImmediateScheduler)));
static SCHEDULER_SET: AtomicBool = AtomicBool::new(false);

type Job = Box<dyn FnOnce() + Send + 'static>;
type JobQueue = Mutex<VecDeque<Job>>;

static QUEUE: LazyLock<JobQueue> = LazyLock::new(|| Mutex::new(VecDeque::new()));

pub fn set_scheduler(scheduler: impl Scheduler) {
    SCHEDULER_SET.store(true, Ordering::SeqCst);
    *SCHEDULER.write().expect("scheduler poisoned") = Arc::new(scheduler);
}

/// Install `scheduler` only if no scheduler was explicitly set yet. Returns
/// whether it was installed. Lets an environment bridge provide a default
/// without overriding an application's choice.
pub fn set_scheduler_if_unset(scheduler: impl Scheduler) -> bool {
    if SCHEDULER_SET.swap(true, Ordering::SeqCst) {
        return false;
    }
    *SCHEDULER.write().expect("scheduler poisoned") = Arc::new(scheduler);
    true
}

pub(crate) fn schedule(job: impl FnOnce() + Send + 'static) {
    QUEUE
        .lock()
        .expect("scheduler queue poisoned")
        .push_back(Box::new(job));

    let scheduler = Arc::clone(&SCHEDULER.read().expect("scheduler poisoned"));
    scheduler.schedule(Box::new(flush));
}

pub fn flush() {
    while let Some(job) = QUEUE.lock().expect("scheduler queue poisoned").pop_front() {
        job();
    }
}
