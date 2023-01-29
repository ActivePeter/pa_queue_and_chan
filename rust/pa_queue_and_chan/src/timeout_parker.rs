
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::AtomicUsize;


use std::sync::atomic::Ordering::SeqCst;
use std::thread;
use std::time::Duration;
/// referred to tokio Parker
#[derive(Debug)]
pub(crate) struct Parker {
    inner: Arc<Inner>,
}
#[derive(Debug)]
pub(crate) struct Unparker {
    inner: Arc<Inner>,
}

pub(crate) fn new_park_pair() -> (Parker, Unparker) {
    let parker=Parker::new();
    let unparker=Unparker::new_from_parker(&parker);
    (parker,unparker)
}
#[derive(Debug)]
struct Inner {
    /// Avoids entering the park if possible
    state: AtomicUsize,

    /// Used to coordinate access to the driver / condvar
    mutex: Mutex<()>,

    /// Condvar to block on if the driver is unavailable.
    condvar: Condvar,

}

const EMPTY: usize = 0;
const PARKED_CONDVAR: usize = 1;
const NOTIFIED: usize = 3;

impl Parker {
    pub(crate) fn new() -> Parker {
        Parker {
            inner: Arc::new(Inner {
                state: AtomicUsize::new(EMPTY),
                mutex: Mutex::new(()),
                condvar: Condvar::new(),
            }),
        }
    }

    pub(crate) fn park_timeout(&mut self, duration: Duration) {
        self.inner.park(duration);
    }

}

impl Unparker {
    pub(crate) fn new_from_parker(parker:&Parker) -> Unparker {
        Unparker{
            inner:Arc::clone(&parker.inner)
        }
    }
    pub(crate) fn unpark(&self) {
        self.inner.unpark();
    }
}

impl Inner {
    /// Parks the current thread for at most `dur`.
    fn park(&self,duration: Duration) {
        for _ in 0..3 {
            // If we were previously notified then we consume this notification and
            // return quickly.
            if self
                .state
                .compare_exchange(NOTIFIED, EMPTY, SeqCst, SeqCst)
                .is_ok()
            {
                return;
            }

            thread::yield_now();
        }

        self.park_condvar(duration);
    }

    /// park_condvar
    fn park_condvar(&self,duration: Duration) {
        // Safely lock the start wait stage.
        let m = match self.mutex.lock(){
            Ok(lock) => {lock}
            Err(e) => {panic!("{}",e)}
        };

        match self
            .state
            .compare_exchange(EMPTY, PARKED_CONDVAR, SeqCst, SeqCst)
        {
            Ok(_) => {}
            Err(NOTIFIED) => {
                // We must read here, even though we know it will be `NOTIFIED`.
                // This is because `unpark` may have been called again since we read
                // `NOTIFIED` in the `compare_exchange` above. We must perform an
                // acquire operation that synchronizes with that `unpark` to observe
                // any writes it made before the call to unpark. To do that we must
                // read from the write it made to `state`.
                let old = self.state.swap(EMPTY, SeqCst);
                debug_assert_eq!(old, NOTIFIED, "park state changed unexpectedly");

                return;
            }
            Err(actual) => panic!("inconsistent park state; actual = {}", actual),
        }

        // this stage holding the lock to avoid unpark send notify.

        match self.condvar.wait_timeout(m,duration){
            Ok(_) => {
                self.state.store(EMPTY,SeqCst);
            }
            Err(e) => {panic!("{}",e)}
        }
    }

    /// decide unpark by current state
    fn unpark(&self) {
        // To ensure the unparked thread will observe any writes we made before
        // this call, we must perform a release operation that `park` can
        // synchronize with. To do that we must write `NOTIFIED` even if `state`
        // is already `NOTIFIED`. That is why this must be a swap rather than a
        // compare-and-swap that returns if it reads `NOTIFIED` on failure.
        match self.state.swap(NOTIFIED, SeqCst) {
            NOTIFIED|EMPTY => {}    // no one was waiting/already unparked
            PARKED_CONDVAR => self.unpark_condvar(),
            actual => panic!("inconsistent state in unpark; actual = {}", actual),
        }
    }

    /// real unpark
    fn unpark_condvar(&self) {
        // There is a period between when the parked thread sets `state` to
        // `PARKED` (or last checked `state` in the case of a spurious wake
        // up) and when it actually waits on `cvar`. If we were to notify
        // during this period it would be ignored and then when the parked
        // thread went to sleep it would never wake up. Fortunately, it has
        // `lock` locked at this stage so we can acquire `lock` to wait until
        // it is ready to receive the notification.
        //
        // Releasing `lock` before the call to `notify_one` means that when the
        // parked thread wakes it doesn't get woken only to have to wait for us
        // to release `lock`.
        drop(self.mutex.lock());

        self.condvar.notify_one();
    }

}