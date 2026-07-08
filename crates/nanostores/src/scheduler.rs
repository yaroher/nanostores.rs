use std::collections::VecDeque;
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

type Job = Box<dyn FnOnce() + Send + 'static>;
type JobQueue = Mutex<VecDeque<Job>>;

static QUEUE: LazyLock<JobQueue> = LazyLock::new(|| Mutex::new(VecDeque::new()));

pub fn set_scheduler(scheduler: impl Scheduler) {
    *SCHEDULER.write().expect("scheduler poisoned") = Arc::new(scheduler);
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
