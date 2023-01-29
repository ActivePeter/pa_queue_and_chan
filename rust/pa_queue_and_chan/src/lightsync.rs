use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::atomic::Ordering::{Acquire, Relaxed};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::thread::{sleep, spawn, Thread};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[inline]
pub(crate) fn current_thread_id() -> u64 {
    // `u8` is not drop so this variable will be available during thread destruction,
    // whereas `thread::current()` would not be
    thread_local! { static DUMMY: u8 = 0 }
    DUMMY.with(|x| {
        let x=x as *const u8;
        x as u64
    })
}

struct LightSync{
    waiting:AtomicBool,
    wait_thread:RwLock<(AtomicU64,Option<Thread>)>
}
impl LightSync{
    pub(crate) fn new() -> LightSync {
        LightSync{
            waiting: AtomicBool::new(false),
            wait_thread: Default::default(),
        }
    }
    pub(crate) fn notify(&self){
        if self.waiting.swap(false,Acquire){
            // println!("wake");
            self.wait_thread.read().unwrap().1.as_ref().unwrap().unpark();
        }
    }

    pub(crate) fn wait(&self){
        if self.wait_thread.read().unwrap().1.is_none()||
            self.wait_thread.read().unwrap().0.load(Relaxed)!=current_thread_id(){
            let mut wr=self.wait_thread.write().unwrap();
            let _=wr.0.swap(current_thread_id(),Acquire);
            let _=wr.1.replace(thread::current());
        }
        if self.waiting.swap(true,Relaxed){
            panic!()
        }
        while self.waiting.load(Relaxed) {
            thread::park();
        }
    }
}

#[cfg(test)]
mod tests{

    #[test]
    pub(crate) fn test_speed(){
        let sync=Arc::new(LightSync::new());
        let handle={
            let sync=sync.clone();
            let handle=spawn(move ||{
                sync.wait();
                // let start_ms=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
                sync.wait();

                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros()
            });
            handle
        };


        sleep(Duration::from_secs(1));
        sync.notify();

        sleep(Duration::from_secs(1));
        let start_notify=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros();
        sync.notify();
        let recv_notify=handle.join().unwrap();
        println!("notify cost {}",recv_notify-start_notify);
    }

    #[test]
    pub(crate) fn test_speed_cond(){
        let sync=Arc::new(Condvar::new());
        let handle={
            let sync=sync.clone();
            let handle=spawn(move ||{
                let mu=Mutex::new(());
                {
                    let lock=mu.lock().unwrap();
                    let _unused = sync.wait(lock).unwrap();
                }
                // let start_ms=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
                {

                    let lock=mu.lock().unwrap();
                    let _unused = sync.wait(lock).unwrap();
                }
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros()
            });
            handle
        };
        sleep(Duration::from_secs(1));
        sync.notify_all();
        sleep(Duration::from_secs(1));
        let start_notify=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros();
        sync.notify_all();
        let recv_notify=handle.join().unwrap();
        println!("cond notify cost {}",recv_notify-start_notify);
    }


    #[test]
    pub(crate) fn test_notify(){
        let sync=Arc::new(LightSync::new());
        for _ in 0..100000000 {
            sync.notify();
        }

        // let recv_notify=handle.join().unwrap();
        // println!("notify cost {}",recv_notify-start_notify);
    }

    #[test]
    pub(crate) fn test_notify_cond(){
        let sync=Arc::new(Condvar::new());
        for _ in 0..100000000 {
            sync.notify_all();
        }
        // println!("cond notify cost {}",recv_notify-start_notify);
    }

    #[test]
    pub(crate) fn test_notify_mt(){
        let sync=Arc::new(LightSync::new());
        for _ in 0..8{
            for _ in 0..10000000 {
                sync.notify();
            }
        }

        // let recv_notify=handle.join().unwrap();
        // println!("notify cost {}",recv_notify-start_notify);
    }

    #[test]
    pub(crate) fn test_notify_cond_mt(){
        let sync=Arc::new(Condvar::new());
        for _ in 0..8{
            for _ in 0..10000000 {
                sync.notify_all();
            }
        }

        // println!("cond notify cost {}",recv_notify-start_notify);
    }
}
