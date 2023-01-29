use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Formatter};
use std::ops::Add;
// use std::ops::{Add};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::{Acquire, Release};
use crossbeam::queue::SegQueue;
use smallvec::SmallVec;
use event_listener::{Event,EventListener};

/// key vec type, can be easily changed
type KeyVec<K> =SmallVec<[K;2]>;

/// create the sender and receiver of channel
#[must_use]
#[inline]
pub fn channel<K,V>() -> (Sender<K, V>, Receiver<K, V>)
where K:Ord+Clone
{
    let chan=Arc::new(Channel::<K,V>::new());

    (Sender::new(Arc::clone(&chan)), Receiver::new(chan))
}


/// Waiting Msg, hold key states ref
struct WaitingMsg<K:Ord+Clone,V>{
    /// msg ks and v, easy to take
    ks_v:Option<(KeyVec<KeyStateShared<K,V>>,V)>,
    // ks_using:KeyVec<bool>,

    /// id of waiting msg
    id:WaitingMsgId
}


impl<K:Ord+Clone,V> WaitingMsg<K,V> {
    /// add k when creating `WaitingMsg`
    fn add_kstate(&mut self,k:KeyStateShared<K,V>){
        if let Some(ks_v)=self.ks_v.as_mut(){
            ks_v.0.push(k);
        }else{panic!()}
    }
    /// take out inner to use, cant be called twice
    fn take_kv(&mut self) -> (KeyVec<K>, V) {
        if let Some((mut kstates,v))=self.ks_v.take(){
            let mut ks =KeyVec::new();
            while let Some(k) =kstates.pop(){
                if let Some(k)=k.inner.borrow_mut().k.as_ref(){
                    ks.push(k.clone());
                }else{panic!()}
            }
            return (ks,v);
        }
        //must success, or there's bug
        panic!()
    }
}

/// Waiting Msg Id
pub(crate) type WaitingMsgId=i32;

/// share between waiting msgs and `key_states` map
struct KeyStateShared<K:Ord+Clone,V>{
    /// inner_
    inner:Rc<RefCell<KeyStateInner<K,V>>>
}

impl<K:Ord+Clone,V> Clone for KeyStateShared<K,V> {
    fn clone(&self) -> Self {
        KeyStateShared{
            inner:Rc::clone(&self.inner)
        }
    }
}

impl<K:Ord+Clone,V> KeyStateShared<K,V> {

    /// create `KeyStateShared` for k
    fn new(k:K) -> KeyStateShared<K,V> {
        KeyStateShared{
            inner: Rc::new(RefCell::new(
                KeyStateInner {
                    k: Some(k),
                    using: false,
                    // ref_cnt: 0,
                    waitingmsgs: HashMap::new(),
                })),
        }
    }

    /// add waiting by weak ref
    fn add_waitingmsg(&self,weak:Weak<RefCell<WaitingMsg<K,V>>>){
        let id=if let Some(b)=weak.upgrade(){
            b.borrow().id
        }else { panic!() };

        let _a=self.inner.borrow_mut().waitingmsgs.insert(id,weak);
    }

    /// remove waiting by id
    fn remove_waitingmsg(&self,id:WaitingMsgId){
        assert!(
            self.inner.borrow_mut().waitingmsgs.remove(&id).is_some());
    }

    /// `try_get_stop_waiting_msg` called, avoid borrow conflict
    fn try_set_use(&self){
        // assert!(!self.inner.borrow().using);
        if let Ok(mut ok) =self.inner.try_borrow_mut(){
            assert!(!ok.using);
            ok.using=true;
        }
    }
    /// set use
    fn set_use(&self){
        assert!(!self.inner.borrow().using);
        self.inner.borrow_mut().using=true;
    }
    /// set unuse
    fn set_unuse(&self){
        assert!(self.inner.borrow().using);
        self.inner.borrow_mut().using=false;
    }
    /// is key using
    fn using(&self) -> bool {
        self.inner.borrow().using
    }
    /// get no conflict one msg and become active message to return to user
    fn try_get_stop_waiting_msg(&self) -> Option<WaitingMsgId> {
        let mut findid =None;
        'outer:for (id,msg) in &self.inner.borrow().waitingmsgs{
            if let Some(msg)=msg.upgrade(){
                if let Some(ks_v)= msg.borrow().ks_v.as_ref(){
                    for k in &ks_v.0{
                        if k.using(){
                            continue 'outer;
                        }
                    }
                    // use this msg
                    for k in &ks_v.0{
                        // k maybe self
                        k.try_set_use();
                    }
                    findid=Some(*id);
                    break 'outer;
                }panic!()
            }else{panic!()}
        }
        if let Some(findid)=findid{
            self.set_use();
            self.remove_waitingmsg(findid);
            return Some(findid);
        }
        None
    }
}

/// inner of `KeyStateShared`
struct KeyStateInner<K:Ord+Clone,V>{
    /// for easily take
    k:Option<K>,
    /// key is hold by active message
    using:bool,
    // ref_cnt:usize,
    /// waiting msgs contains key
    waitingmsgs:HashMap<WaitingMsgId,Weak<RefCell<WaitingMsg<K,V>>>>
}


/// `ConflictManager` handle conflict stuffs
pub(crate) struct ConflictManager<K:Ord+Clone,V>{
    /// waiting messages
    waitingmsgs:HashMap<WaitingMsgId,Rc<RefCell<WaitingMsg<K,V>>>>,

    // key_waitingmsgs:BTreeMap<K,HashMap<WaitingMsgId,Rc<RefCell<WaitingMsg<K,V>>>>>,
    // using_keys:BTreeSet<K>,

    /// key - key states
    keys_states:BTreeMap<K,KeyStateShared<K,V>>,
    /// next_waitingmsg_id
    next_waitingmsg_id:WaitingMsgId
}

impl<K:Ord+Clone,V> Debug for ConflictManager<K,V> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConflictManager")
            .finish()
    }
}

impl<K:Ord+Clone,V> ConflictManager<K,V>{

    /// create `ConflictManager`
    pub(crate) fn new() -> ConflictManager<K, V> {
        ConflictManager{
            waitingmsgs: HashMap::default(),
            // key_waitingmsgs: BTreeMap::default(),
            // using_keys: BTreeSet::default(),
            keys_states:BTreeMap::default(),
            next_waitingmsg_id: 0,
        }
    }

    /// pop drop key and check waiting msg
    pub(crate) fn unuse_all_droped_key(&mut self, dropq: &SegQueue<K>) -> Option<(KeyVec<K>, V)> {
        while let Some(k)=dropq.pop(){
            let keynoref=if let Some(state)=self.keys_states.get(&k){
                state.set_unuse();
                // find msg can be took
                if let Some(id)=state.try_get_stop_waiting_msg(){
                    if let Some(msg)=self.waitingmsgs.remove(&id){
                        return Some(msg.borrow_mut().take_kv());
                    }panic!()
                }
                state.inner.borrow().waitingmsgs.is_empty()
            }else{panic!()};
            if keynoref{ let _a=self.keys_states.remove(&k);}
        }
        None
    }

    /// check is there any waitingmsgs
    #[inline]
    pub(crate) fn waitingmsgs_exist(&self) -> bool {
        !self.waitingmsgs.is_empty()
    }

    /// create new waiting msg
    fn new_waiting_msg(&mut self, v:V) -> WaitingMsg<K, V> {
        let kstates =KeyVec::<KeyStateShared<K,V>>::new();
        let ret=WaitingMsg{
            ks_v: Some((kstates,v)),
            id: self.next_waitingmsg_id,
        };
        self.next_waitingmsg_id=self.next_waitingmsg_id.add(1);

        ret
    }

    /// pop new msg from q
    pub(crate) fn handle_new_msg(&mut self, mut ks:KeyVec<K>, v:V) -> Option<(KeyVec<K>, V)> {
        let mut v =Some(v);
        let mut new_waitingmsg =None;
        for k in &ks{
            //waiting msg hold,or using
            if let Some(keystate)=self.keys_states.get(k){
                if keystate.using(){
                    if let Some(v)=v.take(){
                        new_waitingmsg=Some(self.new_waiting_msg(v));
                        break;
                    } panic!() } } }
        //become waiting
        if let Some(new_waitingmsg)=new_waitingmsg.take(){
            let new_waitingmsg=Rc::new(RefCell::new(new_waitingmsg));
            // all key map into keys_states and
            while let Some(k)=ks.pop() {
                let newkeystate=self.keys_states.entry(k.clone()).or_insert_with(||KeyStateShared::new(k));
                new_waitingmsg.borrow_mut().add_kstate(newkeystate.clone());
                newkeystate.add_waitingmsg(Rc::downgrade(&new_waitingmsg));
            }
            let id=new_waitingmsg.borrow().id;
            assert!(self.waitingmsgs.insert(id,new_waitingmsg).is_none());
            None
        }else{
            //become using
            // + 新消息返回,keys成为using keys,同步到waitingmsgs（消息key数（S） * 搜索key_waitingmsgs （logQ））
            for k in &ks{
                let newkeystate=self.keys_states.entry(k.clone()).or_insert_with(||KeyStateShared::new(k.clone()));
                newkeystate.set_use();
                // newkeystate.add_ref();
            }
            //
            if let Some(v)=v.take(){
                Some((ks,v))
            }else{ panic!() }
        }
    }
}

#[derive(Debug)]
/// channel, inner datas, shared by Sender, Receiver and Message
pub(crate) struct Channel<K:Ord+Clone,V> {
    /// wake_recv_event
    wake_recv_event:Event,
    /// queue for senders to send to receiver
    q:SegQueue<(KeyVec<K>, V)>,
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
            wake_recv_event: Event::new(),
            q: SegQueue::default(),
            drop_key_q: SegQueue::default(),
            senders_dropped: AtomicBool::new(false),
            receiver_dropped: AtomicBool::new(false),
        }
    }

    /// called by sender
    pub(crate) fn inner_send(&self, ks: KeyVec<K>, v:V){
        self.q.push((ks,v));
        self.wake_recv_event.notify(1);
    }

    /// called by receiver, sync until there's data to take out or the senders all dropped
    pub(crate) fn sync_pop(
        &self,
        listener: &mut Option<EventListener>,
        conflict_manager:&mut ConflictManager<K,V>,
        // using_keys:&mut BTreeSet<K>,
    ) -> Option<(KeyVec<K>, V)> {
        'try_pop_once: loop {
            {// take out the dropped keys from queue，and try get freed waiting msg
                if let Some(freed)=conflict_manager.unuse_all_droped_key(&self.drop_key_q){
                    return Some(freed);
                }
            }
            match self.q.pop(){
                None => {
                    // after listen, all new event will be captured
                    // if queue size changed before listen，we should avoid wait
                    if !self.q.is_empty()||!self.drop_key_q.is_empty(){
                        continue 'try_pop_once;
                    }
                    // no data to take out and senders all dropped,
                    //   return none to end
                    if !conflict_manager.waitingmsgs_exist()&&
                        self.senders_dropped.load(Acquire){
                        return None;
                    }

                    //unwrap, listener is always ready
                    match listener.take() {
                        Some(l)=>{
                            // or we will wait
                            l.wait();
                            let _b = listener.insert(self.wake_recv_event.listen());
                        }
                        None=>{panic!()}
                    }
                }
                Some((ks,v)) => {
                    if let Some(res)=conflict_manager.handle_new_msg(ks,v){
                        // return
                        return Some(res);
                    }
                    // conflict, became waiting msg
                    continue 'try_pop_once;

                    // msg has using key, push it into waiting queue.
                    // for k in &ks {
                    //     if using_keys.contains(k){
                    //         let _=waitingmsgs.add_msg((ks,v));
                    //         // msgs_wait_for_k.push_back(msgid);
                    //         continue 'try_pop_once;
                    //     }
                    // }
                    // // msg can be take out, record the key
                    // for k in &ks{
                    //     assert!(using_keys.insert(k.clone()));
                    // }
                    // return Some((ks,v));
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
//clone able
#[derive(Clone,Debug)]
/// sender, cloneable, multi producer
pub struct Sender<K:Ord+Clone,V>{
    // chan:Arc<Channel<K,V>>
    /// inner
    inner:Arc<SenderInner<K,V>>
}

impl<K:Ord+Clone,V> Sender<K,V> {
    /// inner create
    pub(crate) fn new(chan:Arc<Channel<K,V>>) -> Sender<K, V> {
        Sender{
            inner: Arc::new( SenderInner {
                chan,
            }),
        }
    }

    /// # Errors
    ///
    /// send message
    #[inline]
    pub fn send(&self, k: KeyVec<K>, v:V) ->Result<(),SendResFail>{
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
    pub(crate) fn send(&self, ks: KeyVec<K>, v:V) ->Result<(),SendResFail>{
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
        self.chan.senders_dropped.store(true,Release);
        self.chan.wake_recv_event.notify(1);
    }
}

//cant clone
#[derive(Debug)]
/// single consumer, which cannot be cloned.
pub struct Receiver<K:Ord+Clone,V>{
    /// inner channel
    chan:Arc<Channel<K,V>>,
    /// msgs waiting for conflict keys
    conflict_manager: ConflictManager<K,V>,
    /// keys being used and msgs waiting for key
    // using_keys:BTreeSet<K>,
    /// listner
    event_listener:Option<EventListener>
}

impl<K:Ord+Clone,V> Receiver<K,V>{
    /// make chan 创建 receiver
    pub(crate) fn new(chan:Arc<Channel<K,V>>) -> Receiver<K, V> {
        let event_listener=chan.wake_recv_event.listen();
        Receiver{
            chan,
            conflict_manager:ConflictManager::new(),
            // using_keys: BTreeSet::default(),
            event_listener:Some(event_listener)
        }
    }
    #[inline]
    /// # Errors
    ///
    /// call sync pop and return Result
    pub fn recv(&mut self) -> Result<Message<K, V>,RecvResFails> {
        match self.chan.sync_pop(&mut self.event_listener,&mut self.conflict_manager) {
            None => {
                Err(RecvResFails::Closed)
            }
            Some((ks,v)) => {
                Ok(Message::new(v,Arc::clone(&self.chan),ks))
            }
        }
    }
}
impl<K:Ord+Clone,V> Drop for Receiver<K,V>{
    #[inline]
    fn drop(&mut self) {
        self.chan.receiver_dropped.store(true,Release);
        self.chan.wake_recv_event.notify(1);
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
    ks:KeyVec<K>
}

impl<K:Ord+Clone,V> Message<K,V> {
    /// new default msg
    pub(crate) fn new(v:V,
                      chan:Arc<Channel<K,V>>,
                      ks: KeyVec<K>) -> Message<K, V> {
        Message{
            v,chan,ks
        }
    }
}
impl<K:Ord+Clone,V> Drop for Message<K, V>  {
    #[inline]
    fn drop(&mut self) {
        while let Some(k)=self.ks.pop(){
            self.chan.drop_key_q.push(k);
        }
        self.chan.wake_recv_event.notify(1);

    }
}

#[cfg(test)]
mod tests{
    use std::cmp::min;
    use std::collections::HashSet;
    use std::thread::sleep;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use rand::Rng;
    use smallvec::smallvec;
    use crate::kv_mpsc_multik;
    use crate::kv_mpsc_multik::{ConflictManager, Message, RecvResFails};


    #[test]
    fn compare_freed_waitingmsg_search_algo(){
        let mut conf =ConflictManager::new();
        let mut set=HashSet::new();
        // let there be some using keys
        for i in 0..10000{
            let _=conf.handle_new_msg(smallvec![i,i+1,i+2],1);
            let _=set.insert(i);
            let _=set.insert(i+1);
            let _=set.insert(i+2);
        }
        // let there be some waiting msgs
        for i in 0..10000{
            let _=conf.handle_new_msg(smallvec![i,i+1,i+2],1);
        }
        // new algo, we only search dropped key
        // we suppose 50,51,52 were dropped
        let mut start_ms=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        for _ in 0..100 {
            for k in 50..53 {
                if let Some(state)=conf.keys_states.get(&k){
                    let _=state.try_get_stop_waiting_msg();
                }else{panic!()}
            }
        }
        println!("key aware algo seach freed waiting cost {}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()-start_ms);

        start_ms=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
        for _ in 0..100 {
            // old algo, we search all msg
            for (_,msg) in &conf.waitingmsgs{
                if let Some(ks_v) =msg.borrow().ks_v.as_ref(){
                    for k in &ks_v.0{
                        let k=k.inner.borrow();
                        if let Some(k)=k.k.as_ref(){
                            let _a=set.contains(k);
                        }
                    }
                }
            }
        }
        println!("old algo seach freed waiting cost {}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()-start_ms);
    }
    //basic no conflict test;
    #[test]
    fn test_basic() {
        println!("test start");
        let (tx, mut rx)=kv_mpsc_multik::channel::<i32,i32>();
        for i in 0..7 {
            let tx=tx.clone();
            let _=std::thread::spawn(move ||{
                for j in 0..100000 {
                    let k=smallvec![j];
                    tx.send(k,j).unwrap();
                }
                println!("thread {} end",i)
            });
        }
        {
            let _ccc=tx;
        }
        let mut cnt=0;
        // let mut res=smallvec![];
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
        // let mut res=smallvec![];
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
        let (tx, rx)=kv_mpsc_multik::channel::<i32,i32>();

        drop(rx);
        for _i in 0..7 {
            let tx=tx.clone();
            let _=std::thread::spawn(move ||{
                for j in 0..100000 {

                    let k=smallvec![j];
                    // println!("thread {} send {}",i,j);
                    assert!(tx.send(k,j).is_err());
                }
            });
        }
    }

    //test recv after senders dropped;
    #[test]
    fn recv_after_no_sender(){
        let (tx, mut rx)=kv_mpsc_multik::channel::<i32,i32>();

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
        let (tx, mut rx)=kv_mpsc_multik::channel::<i32,i32>();
        for i in 0..7 {
            let tx=tx.clone();
            let _=std::thread::spawn(move ||{
                for j in 0..100000 {
                    // println!("thread {} send {}",i,j);

                    let k=smallvec![j];
                    tx.send(k,j).unwrap();
                }
                println!("thread {} end",i)
            });
        }
        drop(tx);
        let mut cnt=0;
        // let mut res=smallvec![];
        let (recycle_tx,recycle_rx)=std::sync::mpsc::channel::<Message<i32,i32>>();
        let _recyle=std::thread::spawn(move ||{
            let mut reses =vec![];
            loop {
                match recycle_rx.recv_timeout(Duration::from_secs(2)) {
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
        let (tx, mut rx)=kv_mpsc_multik::channel::<i32,i32>();
        let _=std::thread::spawn(move ||{
            tx.send(smallvec![1],1).unwrap();

            tx.send(smallvec![1],1).unwrap();

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
        // sleep(Duration::from_secs(10));

    }

    //check if multikey conflict really worked by time cost
    #[test]
    fn multikey_conflict(){
        let mut conflict_cnt=0;
        let mut no_conflict_cnt=0;

        let mut conflict_cnt_test=0;
        let mut no_conflict_cnt_test=0;

        let mut rand =rand::thread_rng();
        for _ in 0..500{
            let (tx, mut rx)=kv_mpsc_multik::channel::<i32,i32>();

            let mut vec1=smallvec![];
            let mut vec2=smallvec![];
            let conflict=rand.gen_range(0..2);
            {// random generate no conflict key
                let vec1len=rand.gen_range(2..30);
                for i in 0..vec1len{
                    vec1.push(i);
                }
                //move some of vec1 to vec2
                let vec2len=rand.gen_range(1..vec1len);
                for _ in 0..vec2len{
                    vec2.push(vec1.pop().unwrap());
                }
            }
            if conflict==1{
                let change_2_conflict_cnt_max=min(vec1.len(),vec2.len());
                let change_2_conflict_cnt=rand.gen_range(1..change_2_conflict_cnt_max+1);
                for i in 0..change_2_conflict_cnt{
                    vec1[i]=100+i as i32;
                    vec2[i]=100+i as i32;
                }
                conflict_cnt+=1;
                print!("{}",conflict_cnt,);println!();print!("  ");
                for v in &vec1{
                    print!("{},",v);
                }
                print!("  ");println!();
                for v in &vec2{
                    print!("{},",v);
                }
                println!();
            }else{
                no_conflict_cnt+=1;
            }
            tx.send(vec1,1).unwrap();
            tx.send(vec2,1).unwrap();

            let (recycle_tx,recycle_rx)=std::sync::mpsc::channel::<Message<i32,i32>>();
            let _recyle=std::thread::spawn(move ||{
                let a=recycle_rx.recv().unwrap();
                sleep(Duration::from_millis(100));
                drop(a);
            });
            let a=rx.recv().unwrap();
            let start_ms=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
            recycle_tx.send(a).unwrap();
            let _b=rx.recv().unwrap();
            let end_ms=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();

            if end_ms-start_ms>97{
                conflict_cnt_test+=1;
            }else{
                no_conflict_cnt_test+=1;
            }
            // sleep(Duration::from_secs(10));
        }
        assert_eq!(conflict_cnt,conflict_cnt_test);
        assert_eq!(no_conflict_cnt,no_conflict_cnt_test);

    }
}