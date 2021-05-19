//! This module is responsible for buffer reuse. Since we do a lot of
//! allocations of short-lived-but-not-strictly-temporary `Vec<f32>`, this
//! module allows us to reuse those allocations and save ourselves some heap
//! churn.

use concurrent_queue::ConcurrentQueue;
use lazy_static::lazy_static;

lazy_static! {
    static ref QUEUE: ConcurrentQueue<Vec<f32>>
        = ConcurrentQueue::bounded(100);
}

/// Returns an empty `Vec<f32>` with a reasonable amount of room to grow.
pub fn get_buf() -> Vec<f32> {
    match QUEUE.pop().ok() {
        Some(x) => x,
        None => Vec::with_capacity(512),
    }
}

/// Puts this buffer back on the pile.
pub fn finished_with_buf(mut vec: Vec<f32>) {
    vec.clear();
    let _ = QUEUE.push(vec);
    // if the queue was full, well, we had too many buffers anyway
}
