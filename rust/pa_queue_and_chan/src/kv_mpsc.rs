use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::{Acquire, Release};
use crossbeam::queue::SegQueue;
use event_listener::{Event,EventListener};

// use crate::timeout_parker::{new_park_pair, Parker, Unparker};

/// create the sender and receiver of channel
#[must_use]
#[inline]
pub fn channel<K,V>() -> (Sender<K, V>, Receiver<K, V>)
where K:Ord+Clone
{
    let chan=Arc::new(Channel::<K,V>::new());
    (Sender::new(Arc::clone(&chan)), Receiver::new(chan))
}

#[derive(Debug)]
/// channel, inner datas, shared by Sender, Receiver and Message
pub(crate) struct Channel<K:Ord+Clone,V> {
    /// wake_recv_event model
    wake_recv_event:Event,
    /// queue for senders to send to receiver
    q:SegQueue<(K, V)>,
    /// queue for messages to send to receiver to release key
    drop_key_q:SegQueue<K>,
    /// senders dropped flag
    senders_dropped:AtomicBool,
    /// receiver dropped flag
    receiver_dropped:AtomicBool,
}

impl<K:Ord+Clone,V> Channel<K,V>{
    /// inner create channel
    pub(crate) fn new() -> Channel<K, V> {
        Channel{
            q: SegQueue::default(),
            drop_key_q: SegQueue::default(),
            senders_dropped: AtomicBool::new(false),
            receiver_dropped: AtomicBool::new(false),
            wake_recv_event:Event::new()
        }
    }

    /// called by sender
    pub(crate) fn inner_send(&self, ks: K, v:V){
        self.q.push((ks,v));
        self.wake_recv_event.notify(1);
    }

    /// called by receiver, sync until there's data to take out or the senders all dropped
    pub(crate) fn sync_pop(
        &self,
        listener: &mut Option<EventListener>,
        using_keys:&mut BTreeMap<K,VecDeque<(K,V)>>
    ) -> Option<(K, V)> {
        'try_pop_once: loop {
            {// take out the dropped keys from queue
                while let Some(k)= self.drop_key_q.pop() {
                    if let Some(waiting)=using_keys.get_mut(&k){
                        // if there are messages waiting for the key, take out one and return
                        if let Some(msg)=waiting.pop_front(){
                            return Some(msg);
                        }
                        // if there's no msg waiting, remove the queue,
                        //   means that the key is not using
                        let _empty_q=using_keys.remove(&k);

                    }else{ panic!() }
                }
            }
            match self.q.pop(){
                None => {
                    // after listen, all new event will be captured
                    // if queue size changed before wait，we should stop wait
                    if !self.q.is_empty()||!self.drop_key_q.is_empty(){
                        continue 'try_pop_once;
                    }
                    // no data to take out and senders all dropped,
                    //   return none to end
                    if using_keys.is_empty()&&
                        self.senders_dropped.load(Acquire){
                        return None;
                    }
                    //unwrap, listener is always ready
                    match listener.take() {
                        Some(l)=>{
                            // or we will wait
                            l.wait();
                            let _a = listener.insert(self.wake_recv_event.listen());
                        }
                        None=>{panic!()}
                    }
                }
                Some((ks,v)) => {
                    // msg has using key, push it into waiting queue.
                    if let Some(waiting_for_k)=using_keys.get_mut(&ks){
                        waiting_for_k.push_back((ks,v));
                        continue 'try_pop_once;
                    }

                    // msg can be take out, record the key
                    assert!(using_keys.insert(ks.clone(),VecDeque::new()).is_none());
                    return Some((ks,v));
                }
            }
        }
    }
}

#[non_exhaustive]
#[derive(Debug,Clone,Copy)]
/// in Error result when channel closed.
pub enum SendResFail{
    /// receiver dropped, channel closed
    Closed,
}

#[derive(Clone,Debug)]
/// sender, cloneable, multi producer
pub struct Sender<K:Ord+Clone,V>{
    // chan:Arc<Channel<K,V>>
    /// inner
    inner:Arc<SenderInner<K,V>>,
}

impl<K:Ord+Clone,V> Sender<K,V> {
    /// inner create
    pub(crate) fn new(chan:Arc<Channel<K,V>>) -> Sender<K, V> {
        Sender{
            inner: Arc::new( SenderInner {
                chan
            }),
        }
    }

    /// # Errors
    ///
    /// send message
    #[inline]
    pub fn send(&self, k: K, v:V) ->Result<(),SendResFail>{
        self.inner.send(k,v)
    }

}

#[derive(Debug)]
/// sender inner for wrapping an Arc in Sender
/// drop when counting zero
pub struct SenderInner<K:Ord+Clone,V>{
    /// innser chan
    chan:Arc<Channel<K,V>>,
}

impl<K:Ord+Clone,V> SenderInner<K,V>  {

    /// send
    #[inline]
    pub(crate) fn send(&self, ks: K, v:V) ->Result<(),SendResFail>{
        if self.chan.receiver_dropped.load(Acquire){
            return Err(SendResFail::Closed);
        }
        self.chan.inner_send(ks,v);
        Ok(())
    }
}

impl<K:Ord+Clone,V> Drop for SenderInner<K,V> {
    #[inline]
    fn drop(&mut self) {
        // prevent user code to move after
        self.chan.senders_dropped.store(true,Release);
        self.chan.wake_recv_event.notify(1);
    }
}

#[derive(Debug)]
/// single consumer, which cannot be cloned.
pub struct Receiver<K:Ord+Clone,V>{
    /// inner channel
    chan:Arc<Channel<K,V>>,
    /// keys being used and msgs waiting for key
    using_keys:BTreeMap<K,VecDeque<(K,V)>>,
    /// listner
    event_listener:Option<EventListener>
}

impl<K:Ord+Clone,V> Receiver<K,V>{
    /// make chan 创建 receiver
    pub(crate) fn new(chan:Arc<Channel<K,V>>) -> Receiver<K, V> {
        let event_listener=chan.wake_recv_event.listen();
        Receiver{
            chan,
            using_keys: BTreeMap::default(),
            event_listener:Some(event_listener)
        }
    }
    #[inline]
    /// # Errors
    ///
    /// call sync pop and return Result
    pub fn recv(&mut self) -> Result<Message<K, V>,RecvResFails> {
        match self.chan.sync_pop(//&mut self.waitingmsgs,
                                 &mut self.event_listener, &mut self.using_keys) {
            None => {
                Err(RecvResFails::Closed)
            }
            Some((k,v)) => {
                Ok(Message::new(v,Arc::clone(&self.chan),k))
            }
        }
    }
}

impl<K:Ord+Clone,V> Drop for Receiver<K,V>{
    #[inline]
    fn drop(&mut self) {
        self.chan.receiver_dropped.store(true,Release);
    }
}

/// return in recv Error result
#[non_exhaustive]
#[derive(Debug,Clone,Copy)]
pub enum RecvResFails{
    /// senders all dropped and channel closed.
    Closed,
}


/// message channel recv, holding the using key
#[derive(Debug)]
pub struct Message<K:Ord+Clone, V> {
    /// msg value
    pub v:V,
    /// inner channel
    chan:Arc<Channel<K,V>>,
    /// msg keys
    k:Option<K>,
}

impl<K:Ord+Clone,V> Message<K,V> {
    /// new default msg
    pub(crate) fn new(
        v:V,
        chan:Arc<Channel<K,V>>,
        k: K,
    ) -> Message<K, V> {
        Message{
            v,chan,
            k:Some(k),
        }
    }
}
impl<K:Ord+Clone,V> Drop for Message<K, V>  {
    #[inline]
    fn drop(&mut self) {

        let mut k =None;
        std::mem::swap(&mut k,&mut self.k);
        if let Some(k)=k{
            //avoid key-cloning, when key is String type, this will be a cost
            self.chan.drop_key_q.push(k);
        }else { panic!() }
        self.chan.wake_recv_event.notify(1);
    }
}


#[cfg(test)]
mod tests{
    use std::thread::sleep;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use crate::kv_mpsc;
    use crate::kv_mpsc::{Message, RecvResFails};

    //basic no conflict test;
    #[test]
    fn test_basic() {
        println!("test start");
        let (tx, mut rx)=kv_mpsc::channel::<i32,i32>();

        for i in 0..7 {
            let tx=tx.clone();
            let _=std::thread::spawn(move ||{
                for j in 0..100000 {
                    // let k=smallvec![j];
                    tx.send(j,j).unwrap();
                }
                println!("thread {} end",i)
            });
        }
        {
            let _ccc=tx;
        }
        let mut cnt=0;
        // let mut res=vec![];
        loop {
            match rx.recv() {
                Ok(_msg) => {
                    cnt+=1;
                    // res.push(msg);
                }
                Err(e) => {
                    match e {
                        RecvResFails::Closed => {
                            break;
                        }
                    }
                }
            }
        }
        println!("{}",cnt);
    }

    //compare time cost to std mpsc
    #[test]
    fn test_basic_compare2std() {
        println!("test start");
        let (tx, rx)=std::sync::mpsc::channel::<i32>();
        let threadcnt=7;
        let each_thread_insert=100000;
        for i in 0..7 {
            let tx=tx.clone();
            let _=std::thread::spawn(move ||{
                for j in 0..100000 {
                    // println!("thread {} send {}",i,j);
                    tx.send(j).unwrap();
                }
                println!("thread {} end",i)
            });
        }
        {
            let _ccc=tx;
        }
        let mut cnt=0;
        // let mut res=vec![];
        loop {
            match rx.recv() {
                Ok(_msg) => {
                    cnt+=1;
                    // res.push(msg);
                }
                Err(e) => {
                    println!("{}",e);
                    break;
                }
            }
        }
        println!("{}",cnt);
        assert_eq!(cnt,threadcnt*each_thread_insert);
    }

    //test send after receiver dropped
    #[test]
    fn send_after_no_receiver(){
        println!("test start");
        let (tx, rx)=kv_mpsc::channel::<i32,i32>();

        drop(rx);
        for _i in 0..7 {
            let tx=tx.clone();
            let _=std::thread::spawn(move ||{
                for j in 0..100000 {

                    // let k=smallvec![j];
                    // println!("thread {} send {}",i,j);
                    assert!(tx.send(j,j).is_err());
                }
            });
        }
    }

    //test recv after senders dropped;
    #[test]
    fn recv_after_no_sender(){
        let (tx, mut rx)=kv_mpsc::channel::<i32,i32>();

        // drop(rx);
        for _i in 0..100 {
            let _tx=tx.clone();
        }
        drop(tx);
        assert!(rx.recv().is_err());
    }


    #[test]
    fn drop_msg_after_a_while(){
        let threadcnt=7;
        let each_thread_insert=100000;

        //transfer message to another thread, and release them when reach threshold or timeout
        println!("test start");
        let (tx, mut rx)=kv_mpsc::channel::<i32,i32>();
        for i in 0..7 {
            let tx=tx.clone();
            let _=std::thread::spawn(move ||{
                for j in 0..100000 {
                    tx.send(j,j).unwrap();
                }
                println!("thread {} end",i)
            });
        }
        drop(tx);
        let mut cnt=0;
        // let mut res=vec![];
        let (recycle_tx,recycle_rx)=std::sync::mpsc::channel::<Message<i32,i32>>();
        let _recyle=std::thread::spawn(move ||{
            let mut reses =vec![];
            loop {
                match recycle_rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(res) => {
                        reses.push(res);
                        if reses.len()==1000{
                            reses.clear();
                        }
                    }
                    Err(_e) => {
                        reses.clear();
                    }
                }
            }

        });
        loop {
            match rx.recv() {
                Ok(msg) => {
                    cnt+=1;
                    // res.push(msg);
                    recycle_tx.send(msg).unwrap();
                }
                Err(e) => {
                    match e {
                        RecvResFails::Closed => {
                            break;
                        }
                    }
                }
            }
        }
        assert_eq!(cnt,threadcnt*each_thread_insert);
    }

    //check if conflict really worked by time cost
    #[test]
    fn check_conflict(){
        let (tx, mut rx)=kv_mpsc::channel::<i32,i32>();
        let _=std::thread::spawn(move ||{
            tx.send(1,1).unwrap();

            tx.send(1,1).unwrap();

        });
        let (recycle_tx,recycle_rx)=std::sync::mpsc::channel::<Message<i32,i32>>();
        let _recyle=std::thread::spawn(move ||{
            let a=recycle_rx.recv().unwrap();
            sleep(Duration::from_secs(10));
            drop(a);
        });
        let a=rx.recv().unwrap();
        let start_ms=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        recycle_tx.send(a).unwrap();
        let _b=rx.recv().unwrap();
        let end_ms=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        assert!(end_ms-start_ms>10000);
        println!("wait {} ms",end_ms-start_ms);

    }
}