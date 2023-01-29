#![cfg_attr(not(feature = "macros"), allow(unreachable_pub))]

//! Asynchronous values.

macro_rules! cfg_process {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "process")]
            #[cfg_attr(docsrs, doc(cfg(feature = "process")))]
            #[cfg(not(loom))]
            #[cfg(not(tokio_wasi))]
            $item
        )*
    }
}

#[cfg(any(feature = "macros", feature = "process"))]
pub(crate) mod maybe_done;

mod poll_fn;
pub use poll_fn::poll_fn;

cfg_process! {
    mod try_join;
    pub(crate) use try_join::try_join3;
}

cfg_sync! {
    mod block_on;
    pub(crate) use block_on::block_on;
}

cfg_trace! {
    mod trace;
    pub(crate) use trace::InstrumentedFuture as Future;
}

cfg_not_trace! {
    cfg_rt! {
        pub(crate) use std::future::Future;
    }
}
