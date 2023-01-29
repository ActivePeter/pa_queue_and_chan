
use std::sync::atomic::{AtomicI32, AtomicPtr};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};

#[derive(Debug)]
pub(crate) struct LockfreeMpscLinkQueue<T>{
    dummyhead:AtomicPtr<Node<T>>,
    tail:AtomicPtr<Node<T>>,
    cnt:AtomicI32
}
impl<T> Default for LockfreeMpscLinkQueue<T>{
    fn default() -> Self {
        LockfreeMpscLinkQueue::new()
    }
}
impl<T> LockfreeMpscLinkQueue<T>{
    pub(crate) fn new()->LockfreeMpscLinkQueue<T>{
        let head=Node::new_head();
        Self{
            dummyhead: AtomicPtr::new(head),
            tail: AtomicPtr::new(head),
            cnt: AtomicI32::new(0),
        }
    }
    pub(crate) fn push(&self, t:T){
        let new=Node::<T>::new(t);
        // std::sync::mpsc::channel();
        loop {
            let oldtail=self.tail.load(Acquire);

            //把当前的tail替换掉，这样其他线程就得接在自己后面
            if
            self.tail.compare_exchange_weak(oldtail,new,Relaxed,Relaxed).is_ok(){
                let _=self.cnt.fetch_add(1,Release);
                unsafe {
                    (*oldtail).next.store(new,Relaxed);
                }
                break;
            }

        }
    }


    pub(crate) fn pop(&self) -> Option<T> {
        if self.cnt.load(Relaxed)==0{
            return None;
        }
        loop {
            //单消费者，出队不用重试，
            unsafe {
                let dummyhead=self.dummyhead.load(Acquire);
                let head=(*dummyhead).next.load(Acquire);
                if head.is_null(){continue;}//等待链表链接完
                (*dummyhead).next.store((*head).next.load(Relaxed),Relaxed);
                let free=Node::free(head);
                let _=self.cnt.fetch_sub(1,Release);
                return Some(free);
            }
        }

    }
}

impl<T> Drop for LockfreeMpscLinkQueue<T> {
    fn drop(&mut self) {
        unsafe {
            loop {
                let head=self.dummyhead.load(Relaxed);
                if (*head).t.is_none(){
                    return;
                }
                let next=(*head).next.load(Relaxed);
                let _=Node::free(head);
                self.dummyhead.store(next, Relaxed);
            }
        }
    }
}
struct Node<T>{
    t:Option<T>,
    next:AtomicPtr<Node<T>>
}

impl<T> Node<T> {
    fn new_head() -> *mut Node<T> {
        Box::into_raw(
            Box::new(
                Node{
                    t:None,
                    next: Default::default(),
                }
            )
        )
    }
    fn new(t:T) -> *mut Node<T> {
        Box::into_raw(
            Box::new(
                Node{
                    t:Some(t),
                    next: Default::default(),
                }
            )
        )
    }
    fn free(node:*mut Node<T>)->T{
        let mut t =None;
        unsafe {
            std::mem::swap(&mut (*node).t,&mut t);
            let _ = Box::from_raw(node);
        }
        t.unwrap()
    }
}

#[cfg(test)]
mod tests{
    use std::sync::Arc;
    use std::sync::atomic::AtomicI32;
    use std::sync::atomic::Ordering::{ Relaxed, Release};
    // use std::thread;
    use crate::lockfree_link_queue::LockfreeMpscLinkQueue;

    #[test]
    fn test() {
        let q:LockfreeMpscLinkQueue<i32>=LockfreeMpscLinkQueue::new();
        let q=Arc::new(q);

        let cnt=Arc::new(AtomicI32::new(0));

        // std::thread::scope(|s|{
        //     let cnt=cnt.clone();
        let mut threads =vec![];
        for i in 0..8 {
            let q=q.clone();
            let cnt=cnt.clone();
            threads.push(std::thread::spawn(move ||{
                for _j in i*100..(i+1)*100{
                    q.push(i);
                }
                let _=cnt.fetch_add(1,Release);
            }));
        }
        // for t in threads{
        //     t.join().unwrap();
        // }

        let mut vec =vec![];
        loop {
            match q.pop() {
                None => {
                    if cnt.load(Relaxed)==8{
                        break;
                    }
                }
                Some(v) => {
                    vec.push(v);
                }
            }
        }
        assert_eq!(vec.len(), 800)
        // });

    }
}