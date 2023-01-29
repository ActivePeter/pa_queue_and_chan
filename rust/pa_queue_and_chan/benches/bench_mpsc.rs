use std::time::Duration;
use criterion::{Criterion, criterion_group, criterion_main};
use smallvec::smallvec;
use mpsctest::{kv_mpsc, kv_mpsc_multik};
use mpsctest::kv_mpsc::RecvResFails;

pub fn bench_mpsc(c: &mut Criterion) {
    c.bench_function("单键无冲突连续发送", |bencher|{
        bencher.iter(||{
            // println!("test start");
            let (tx, mut rx)=kv_mpsc::make_chan::<i32,i32>();
            for i in 0..7 {
                let tx=tx.clone();
                let _=std::thread::spawn(move ||{
                    for j in 0..10000 {
                        // let k=smallvec![j];
                        tx.send(j,j).unwrap();
                    }
                    // println!("thread {} end",i)
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
                            kv_mpsc::RecvResFails::Closed => {
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            // println!("{}",cnt);
        })
    });

    c.bench_function("多键无冲突连续发送", |bencher|{
        bencher.iter(||{
            // println!("test start");
            let (tx, mut rx)=kv_mpsc_multik::make_chan::<i32,i32>();
            for i in 0..7 {
                let tx=tx.clone();
                let _=std::thread::spawn(move ||{
                    for j in 0..10000 {
                        let k=smallvec![j];
                        tx.send(k,j).unwrap();
                    }
                    // println!("thread {} end",i)
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
                            kv_mpsc_multik::RecvResFails::Closed => {
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            // println!("{}",cnt);
        });}

    );
    c.bench_function("单键只发送", |bencher|{
        bencher.iter(||{
            // println!("test start");
            let (tx, mut rx)=kv_mpsc::make_chan::<i32,i32>();
            let mut threads =vec![];
            for i in 0..7 {
                let tx=tx.clone();
                let t=std::thread::spawn(move ||{
                    for j in 0..10000 {
                        // let k=smallvec![j];
                        tx.send(j,j).unwrap();
                    }
                    // println!("thread {} end",i)
                });
                threads.push(t);
            }
            {
                let _ccc=tx;
            }
            for t in threads{
                t.join().unwrap();
            }
            // println!("{}",cnt);
        })
    });
    c.bench_function("多键只发送", |bencher|{
        bencher.iter(||{
            // println!("test start");
            let (tx, mut rx)=kv_mpsc_multik::make_chan::<i32,i32>();
            let mut threads =vec![];
            for i in 0..7 {
                let tx=tx.clone();
                let t=std::thread::spawn(move ||{
                    for j in 0..10000 {
                        let k=smallvec![j];
                        tx.send(k,j).unwrap();
                    }
                    // println!("thread {} end",i)
                });
                threads.push(t);
            }
            {
                let _ccc=tx;
            }
            for t in threads{
                t.join().unwrap();
            }
            // println!("{}",cnt);
        })
    });
}

criterion_group!(
    benches,
    bench_mpsc
);

criterion_main!(benches);
