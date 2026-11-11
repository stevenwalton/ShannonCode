//! **Taking input that has already arrived, without breaking the stream.**
//!
//! One function, and it exists because the obvious way to write it deadlocks
//! the entire program after a single keystroke.
//!
//! [`crossterm::event::EventStream`] does not register a waker on every poll.
//! It keeps a latch (`stream_wake_task_executed`): the *first* poll that finds
//! the queue empty clones `cx.waker()`, hands it to a background thread and
//! sets the latch; every later poll that finds the queue empty sees the latch
//! already set and returns `Pending` **without looking at `cx` at all**. The
//! latch clears only when that background thread observes an event and wakes
//! the waker it was given. Whoever polls first therefore decides, for the whole
//! life of that wait, which waker gets woken.
//!
//! `futures::FutureExt::now_or_never` polls with a **no-op** waker. So
//! `now_or_never(stream.next())` — the natural way to ask "is one queued?" —
//! hands the no-op waker to that background thread and latches it in. The next
//! real poll cannot replace it. When a key finally arrives the thread wakes the
//! no-op waker, the executor is never told, and the task that owns the terminal
//! sleeps forever. It is not a dropped frame; it is a permanent deadlock, and
//! `crossterm` compounds it by having that thread hold the internal reader
//! mutex while it waits, so the blocking `event::poll`/`event::read` API cannot
//! be used to dig out either (`poll_internal(Some(_))` gives up on the lock and
//! reports "no event", which is a lie the caller cannot distinguish).
//!
//! Symptom, in case this is ever reintroduced: the app handles **exactly one**
//! event and then ignores every key, `<C-c>`, `<C-l>` and terminal resize.
//!
//! The rule this module exists to keep: **never poll the event stream with a
//! waker that is not the polling task's own.** [`queued_now`] is that poll,
//! written so the borrow it takes is the enclosing task's real context.

use std::future::poll_fn;
use std::pin::Pin;
use std::task::Poll;

use tokio_stream::Stream;

/// The next item **if one is already queued**, without ever waiting.
///
/// - `None` — nothing was ready. The stream has been left armed with *this
///   task's* waker, so the wait that follows will be woken normally.
/// - `Some(item)` — an item was queued; `item` is `None` if the stream ended.
///
/// This is `now_or_never` with the one difference that matters: the poll uses
/// the caller's real [`Context`](std::task::Context) rather than a no-op waker.
/// See the module docs for why that difference is the whole point — and note
/// that it is invisible in a diff, because both spellings return immediately
/// and both are correct for a stream that re-registers on every poll.
pub async fn queued_now<S>(stream: &mut S) -> Option<Option<S::Item>>
where
    S: Stream + Unpin,
{
    poll_fn(|cx| {
        Poll::Ready(match Pin::new(&mut *stream).poll_next(cx) {
            Poll::Ready(item) => Some(item),
            Poll::Pending => None,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::task::Waker;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tokio_stream::StreamExt;

    /// A stream with **`crossterm`'s waker latch**, which is the property under
    /// test. Re-modelled here rather than driving the real `EventStream`
    /// because that one needs a tty: `cargo test` has none, its source fails to
    /// open, and every poll would return an error instead of exercising the
    /// latch at all.
    ///
    /// The three rules copied from `EventStream::poll_next`:
    /// 1. an item in the queue is returned immediately, `cx` untouched;
    /// 2. otherwise the **first** empty poll stores `cx.waker()` and latches;
    /// 3. a later empty poll returns `Pending` and stores nothing — so whoever
    ///    polled first owns the wakeup for the whole wait.
    #[derive(Default)]
    struct Latched {
        inner: Arc<Mutex<Inner>>,
    }

    #[derive(Default)]
    struct Inner {
        queue: VecDeque<u8>,
        waiting: Option<Waker>,
        latched: bool,
        /// Polls that parked. Lets a test wait for the reader to be *parked*
        /// rather than sleeping and hoping — the ordering matters, and getting
        /// it wrong fails in the silent direction (the item is already queued,
        /// so no wakeup is needed and the bug passes).
        parked: usize,
    }

    impl Latched {
        fn handle(&self) -> Arc<Mutex<Inner>> {
            Arc::clone(&self.inner)
        }

        fn with(items: impl IntoIterator<Item = u8>) -> Self {
            let this = Self::default();
            this.inner.lock().expect("fresh mutex").queue.extend(items);
            this
        }
    }

    impl Stream for Latched {
        type Item = u8;

        fn poll_next(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Option<u8>> {
            let mut inner = self.inner.lock().expect("uncontended");
            if let Some(item) = inner.queue.pop_front() {
                return Poll::Ready(Some(item));
            }
            if !inner.latched {
                inner.latched = true;
                inner.waiting = Some(cx.waker().clone());
            }
            inner.parked += 1;
            Poll::Pending
        }
    }

    /// Deliver an item the way `crossterm`'s background thread does: queue it,
    /// clear the latch, and wake **whichever waker was stored** — the bug being
    /// modelled is that this can be a waker nobody is listening to.
    fn deliver(inner: &Arc<Mutex<Inner>>, item: u8) {
        let woken = {
            let mut inner = inner.lock().expect("uncontended");
            inner.queue.push_back(item);
            inner.latched = false;
            inner.waiting.take()
        };
        if let Some(waker) = woken {
            waker.wake();
        }
    }

    /// Spin until the reader has parked `n` times, so the delivery below is
    /// answering a *wait* and not merely filling a queue.
    async fn parked_at_least(inner: &Arc<Mutex<Inner>>, n: usize) {
        for _ in 0..10_000 {
            if inner.lock().expect("uncontended").parked >= n {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the reader never parked");
    }

    /// **The regression.** A drain that comes up empty must leave the stream
    /// able to wake *us*; arming it with a foreign waker instead means the wait
    /// that follows never ends.
    ///
    /// The waiting happens in its **own task with no timer in it**, and that is
    /// the whole design of this test. The first version put a
    /// `tokio::time::timeout` around the wait in the same task — and the
    /// timeout's own wakeup re-polled the stream, found the item sitting in the
    /// queue, and returned it. The bug passed. A mutation check is only as good
    /// as the guarantee that *nothing else* can wake the task
    /// ([[feedback_mutation_validates_the_path]]): here the only thing that may
    /// wake the waiter is the stream itself, so if the wakeup is lost the task
    /// never finishes and `done_rx` never resolves.
    #[tokio::test]
    async fn an_empty_drain_leaves_the_stream_able_to_wake_us() {
        let mut stream = Latched::default();
        let inner = stream.handle();
        let (done_tx, done_rx) = oneshot::channel();

        tokio::spawn(async move {
            assert!(
                queued_now(&mut stream).await.is_none(),
                "nothing was queued, so the drain must report nothing"
            );
            // Exactly what the event loop does next: draw, then wait.
            let _ = done_tx.send(stream.next().await);
        });

        // Both polls are in: the drain's, and the wait's.
        parked_at_least(&inner, 2).await;
        deliver(&inner, 7);

        let arrived = tokio::time::timeout(Duration::from_secs(5), done_rx)
            .await
            .expect("the waiter was never woken: the drain armed the stream with a foreign waker")
            .expect("waiter task panicked");
        assert_eq!(arrived, Some(7));
    }

    /// The drain still drains — otherwise the fix above could be "always report
    /// nothing", which passes the regression test and quietly costs a frame per
    /// keystroke.
    #[tokio::test]
    async fn a_queued_item_is_taken_without_waiting() {
        let mut stream = Latched::with([1, 2]);
        assert_eq!(queued_now(&mut stream).await, Some(Some(1)));
        assert_eq!(queued_now(&mut stream).await, Some(Some(2)));
        assert_eq!(queued_now(&mut stream).await, None, "queue is empty now");
    }
}
