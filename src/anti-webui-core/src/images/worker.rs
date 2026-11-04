//! The threads pictures are decoded on. Nobody using the framework sees them: what they make arrives
//! on the UI thread, in [`super::ImageStore::poll`]. See `docs/design/22-images.md`.

use super::decode;
use super::{Decoded, Info};
use anti_webui_style::ImageSource;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// A thread with nothing to do goes away after this.
const IDLE: Duration = Duration::from_secs(10);

pub(crate) enum Task {
    Probe,
    Decode { target: (u32, u32) },
}

pub(crate) enum Outcome {
    Info(Info),
    Decoded(Decoded),
    Failed(String),
}

pub(crate) struct Done {
    pub source: ImageSource,
    pub target: Option<(u32, u32)>,
    pub outcome: Outcome,
}

/// Where what is done goes.
#[derive(Default)]
pub(crate) struct Reply {
    done: Mutex<Vec<Done>>,
    arrived: Condvar,
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Reply {
    fn send(&self, done: Done) {
        self.done.lock().expect("no panics in here").push(done);
        self.arrived.notify_all();
        let waker = self.waker.lock().expect("no panics in here").clone();
        if let Some(wake) = waker {
            wake();
        }
    }

    pub fn take(&self) -> Vec<Done> {
        std::mem::take(&mut self.done.lock().expect("no panics in here"))
    }

    pub fn put_back(&self, mut rest: Vec<Done>) {
        let mut done = self.done.lock().expect("no panics in here");
        rest.append(&mut done);
        *done = rest;
    }

    pub fn set_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
        *self.waker.lock().expect("no panics in here") = Some(waker.clone());
        if !self.done.lock().expect("no panics in here").is_empty() {
            waker();
        }
    }

    /// Waits until something has arrived, for at most `timeout`.
    pub fn wait(&self, timeout: Duration) {
        let done = self.done.lock().expect("no panics in here");
        if done.is_empty() {
            let _ = self.arrived.wait_timeout(done, timeout);
        }
    }
}

pub(crate) struct Job {
    pub source: ImageSource,
    pub task: Task,
    /// Nobody wants it any more.
    pub cancelled: Arc<AtomicBool>,
    pub reply: Arc<Reply>,
}

#[derive(Default)]
struct Queue {
    /// These only read a little and everything waits for them: they go first.
    probes: VecDeque<Job>,
    /// The latest go first: what was asked for last is what is on the screen by now.
    decodes: Vec<Job>,
    threads: usize,
    idle: usize,
}

#[derive(Default)]
struct Pool {
    queue: Mutex<Queue>,
    work: Condvar,
}

static POOL: OnceLock<Pool> = OnceLock::new();

fn most_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(2, |n| n.get().saturating_sub(1))
        .clamp(1, 4)
}

pub(crate) fn submit(job: Job) {
    let pool = POOL.get_or_init(Pool::default);
    let mut queue = pool.queue.lock().expect("no panics in here");
    match job.task {
        Task::Probe => queue.probes.push_back(job),
        Task::Decode { .. } => queue.decodes.push(job),
    }
    if queue.idle > 0 {
        pool.work.notify_one();
    } else if queue.threads < most_threads() {
        queue.threads += 1;
        drop(queue);
        std::thread::Builder::new()
            .name("anti-webui images".into())
            .spawn(move || run(pool))
            .expect("threads can be made");
    }
}

fn run(pool: &'static Pool) {
    let mut queue = pool.queue.lock().expect("no panics in here");
    loop {
        let job = queue.probes.pop_front().or_else(|| queue.decodes.pop());
        let Some(job) = job else {
            queue.idle += 1;
            let (back, timeout) = pool
                .work
                .wait_timeout(queue, IDLE)
                .expect("no panics in here");
            queue = back;
            queue.idle -= 1;
            if timeout.timed_out() && queue.probes.is_empty() && queue.decodes.is_empty() {
                queue.threads -= 1;
                return;
            }
            continue;
        };
        drop(queue);
        work(job);
        queue = pool.queue.lock().expect("no panics in here");
    }
}

fn work(job: Job) {
    if job.cancelled.load(Ordering::Relaxed) {
        return;
    }
    let target = match job.task {
        Task::Probe => None,
        Task::Decode { target } => Some(target),
    };
    // Decoders are fed whatever is in the file. If one panics on it, that picture has failed.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match target {
        None => decode::probe(&job.source).map(Outcome::Info),
        Some(target) => decode::decode(&job.source, target).map(Outcome::Decoded),
    }))
    .unwrap_or_else(|_| Err("the decoder panicked".into()))
    .unwrap_or_else(Outcome::Failed);
    job.reply.send(Done {
        source: job.source,
        target,
        outcome,
    });
}
