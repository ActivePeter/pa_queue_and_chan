use std::collections::HashMap;
// use std::sync::atomic:: AtomicUsize;
use std::sync:: RwLock;
// use std::sync::atomic::Ordering::{Relaxed, Release};
// use std::thread;
use crossbeam::queue::SegQueue;

/// Returns a unique id for the current thread.
#[inline]
pub(crate) fn current_thread_id() -> usize {
    // `u8` is not drop so this variable will be available during thread destruction,
    // whereas `thread::current()` would not be
    thread_local! { static DUMMY: u8 = 0 }
    DUMMY.with(|x| {
        let x=x as *const u8;
        x as usize
    })
}



//不要求顺序的话，每个线程可以分配单独的queue
#[derive(Debug)]
pub(crate) struct MpscNoOrderQueue<T>{
    threads_queues: RwLock<HashMap<usize,SegQueue<T>>>,
    // len:AtomicUsize
}


impl<T> MpscNoOrderQueue<T> {
    pub(crate) fn new() -> MpscNoOrderQueue<T> {
        return MpscNoOrderQueue{
            // len: AtomicUsize::new(0),
            threads_queues: Default::default(),
        }
    }
    pub(crate) fn push(&self,t:T){
        // std::sync::mpsc::channel()
        let curthreadid=current_thread_id();
        //只有在新线程加入时要获取写锁，其余时刻都是读锁
        if !self.threads_queues.read().unwrap().contains_key(&curthreadid){
            let _=self.threads_queues.write().unwrap().insert(curthreadid,SegQueue::new());
        }
        let read=self.threads_queues.read().unwrap();
        let _ = read.get(&curthreadid).unwrap().push(t);
    }

    pub(crate) fn try_pop(&self)->Option<T>{
        let read=self.threads_queues.read().unwrap();
        for q in read.iter(){
            if let Some(a)=q.1.pop(){
                return Some(a);
            }
        }
        None
    }
}
//
// #[test]
// fn test() {
//     let q:MpscNoOrderQueue<i32>=MpscNoOrderQueue::new();
//     let q=Arc::new(q);
//
//     let cnt=Arc::new(AtomicI32::new(0));
//
//     // std::thread::scope(|s|{
//     //     let cnt=cnt.clone();
//     let mut threads =vec![];
//     for i in 0..8 {
//         let q=q.clone();
//         let cnt=cnt.clone();
//         threads.push(thread::spawn(move ||{
//             for j in i*100..(i+1)*100{
//                 q.push(i);
//             }
//             cnt.fetch_add(1,Release);
//         }));
//     }
//     // for t in threads{
//     //     t.join().unwrap();
//     // }
//
//     let mut vec =vec![];
//     loop {
//         match q.try_pop() {
//             None => {
//                 if cnt.load(Relaxed)==8{
//                     break;
//                 }
//             }
//             Some(v) => {
//                 vec.push(v);
//             }
//         }
//     }
//     assert_eq!(vec.len(), 800)
//     // });
//
// }