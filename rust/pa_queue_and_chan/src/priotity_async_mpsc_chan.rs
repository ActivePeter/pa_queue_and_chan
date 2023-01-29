use std::collections::{BTreeMap};
use std::hash::Hash;
use std::sync::{Arc, RwLock};
use crossbeam::queue::SegQueue;
use std::option::Option::Some;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::atomic::Ordering::{Acquire, Release};
use std::task::{Context, Poll};
use std::task::Poll::{Pending, Ready};
use crate::tokio_transplant::atomic_waker::AtomicWaker;
// use event_listener::{Event,EventListener};
use crate::tokio_transplant::future::poll_fn;

/// new chan tx/rx pair
pub fn new<I,P:Ord+Hash+Clone>() -> (Sender<I, P>, Receiver<I, P>) {
    let chan=Arc::new(PriorityChan::new());
    (Sender::new(Arc::clone(&chan)),Receiver::new(chan))
}

#[derive(Debug)]
struct PriorityQueue<I,P:Ord+Hash+Clone>{
    map:RwLock<BTreeMap<P,SegQueue<I>>>,
}

impl <I,P:Ord+Hash+Clone> PriorityQueue<I,P> {
    fn new()->Self{
        Self{
            map:RwLock::new(BTreeMap::new()),
        }
    }
    fn push(&self,i:I,p:P){
        let mut read =Some(self.map.read().unwrap());
        if !read.as_ref().unwrap().contains_key(&p){
            let _=read.take();
            // write
            let mut write=self.map.write().unwrap();
            let _=write.insert(p.clone(),SegQueue::new());
            write.get(&p).unwrap().push(i);
            return;
        }
        read.unwrap().get(&p).unwrap().push(i);
    }
    fn pop(&self) -> Option<I> {
        let mut read =self.map.read().unwrap();
        for (_p,q) in read.iter(){
            if let Some(i)=q.pop(){
                return Some(i);
            }
        }
        None
    }
}
macro_rules! ready {
    ($e:expr $(,)?) => {
        match $e {
            std::task::Poll::Ready(t) => t,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

#[derive(Debug)]
struct PriorityChan<I,P:Ord+Hash+Clone>{
    queue:PriorityQueue<I,P>,
    end_flag:AtomicBool,
    waker:AtomicWaker,
    // wake_event:Event
}

impl <I,P:Ord+Hash+Clone> PriorityChan<I,P>{
    fn new()->PriorityChan<I, P> {
        Self{
            queue:PriorityQueue::new(),
            end_flag:AtomicBool::new(false),
            waker:AtomicWaker::new(),
            // wake_event:Event::new(),
        }
    }
    // return none means closed
    fn recv(&self, cx: &mut Context<'_>) -> Poll<Option<I>> {
        macro_rules! try_recv {
            () => {
                match self.queue.pop(){
                    None => {
                        // none and closed: means closed
                        if self.end_flag.load(Acquire){
                            return Ready(None);
                        }
                    }
                    Some(v) => {return Ready(Some(v));}
                }
            };
        }
        // if success, we dont need to register waker
        try_recv!();

        self.waker.register_by_ref(cx.waker());


        // refer to tokio:
        //  It is possible that a value was pushed between attempting to read
        //  and registering the task, so we have to check the channel a
        //  second time here.
        try_recv!();

        println!("pending");
        Pending
    }
}

#[derive(Debug)]
struct SenderInner<I,P:Ord+Hash+Clone>{
    chan:Arc<PriorityChan<I,P>>
}

impl<I,P:Ord+Hash+Clone> Drop for SenderInner<I,P> {
    fn drop(&mut self) {
        self.chan.end_flag.store(true,Release);
        // self.chan.wake_event.notify(usize::MAX);
    }
}
/// sender
#[derive(Debug,Clone)]
pub struct Sender<I,P:Ord+Hash+Clone>{
    inner:Arc<SenderInner<I,P>>
}


impl <I,P:Ord+Hash+Clone> Sender<I,P> {
    fn new(chan:Arc<PriorityChan<I,P>>) -> Sender<I, P> {
        let inner=Arc::new(SenderInner{
            chan
        });
        Self{
            inner:inner
        }
    }
    /// send sync
    pub fn send_sync(&self,value:I,priority:P)->Result<(),()>{
        if self.inner.chan.end_flag.load(Acquire){
            return Err(())
        }
        self.inner.chan.queue.push(value,priority);
        self.inner.chan.waker.wake();
        // println!("wake");
        Ok(())
    }
}

///receiver
#[derive(Debug)]
pub struct Receiver<I,P:Ord+Hash+Clone>{
    chan:Arc<PriorityChan<I,P>>
}
impl <I,P:Ord+Hash+Clone> Receiver<I,P> {
    fn new(chan:Arc<PriorityChan<I,P>>) -> Receiver<I, P> {
        Receiver{
            chan,
        }
    }
    ///receive a value
    pub async fn recv(&self) -> Option<I> {
        poll_fn(|cx| self.chan.recv(cx)).await
    }
}

#[cfg(test)]
mod tests{
    use std::time::Duration;

    //basic no conflict test;
    #[tokio::test]
    async fn test_basic(){
        use super::*;
        let cnt=10000;
        let (sender,receiver)=new::<usize,usize>();
        for i in 0..cnt{
            let sender=sender.clone();
            tokio::spawn(async move {
                sender.send_sync(i,cnt-1-i).unwrap();
            });
            // tokio::time::sleep(Duration::from_secs(1)).await;
            // sender.send_sync(i,9-i);
        }
        let res=tokio::spawn(async move {
            for i in 0..cnt {
                let a=receiver.recv().await.unwrap();
                assert_eq!(cnt-1 - a, i)
            }
        }).await;
        res.unwrap();
    }

    #[tokio::test]
    async fn test_sleep(){
        use super::*;
        println!("test sleep");
        let (sender,receiver)=new::<usize,usize>();
        {
            let sender=sender.clone();
            tokio::time::sleep(Duration::from_secs(1)).await;
            tokio::spawn(async move {
                for i in 0..10{
                    sender.send_sync(i, 9 - i).unwrap(); }
            });
            // sender.send_sync(i,9-i);
        }
        // let res=tokio::spawn(async move {
            for i in 0..10 {
                let a=receiver.recv().await.unwrap();
                assert_eq!(9 - a, i)
            }
        // }).await;
        // res.unwrap();
    }
}