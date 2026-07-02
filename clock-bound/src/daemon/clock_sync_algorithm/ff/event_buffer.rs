//! Local and Estimate ring buffers used within a Feed Forward Clock Sync Algorithm

mod local;
pub use local::{FeedError, Local};

mod estimate;
pub use estimate::Estimate;

#[cfg(test)]
pub(crate) mod test_assets {
    use crate::daemon::{event::TscRtt, time::TscCount};

    // Helper struct to implement TscRtt for testing
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TestEvent {
        pub tsc_pre: u64,
        pub tsc_post: u64,
    }

    impl TscRtt for TestEvent {
        fn tsc_post(&self) -> TscCount {
            TscCount::new(self.tsc_post as i64)
        }

        fn tsc_pre(&self) -> TscCount {
            TscCount::new(self.tsc_pre as i64)
        }
    }

    impl TestEvent {
        pub fn new(tsc_pre: u64, tsc_post: u64) -> Self {
            Self { tsc_pre, tsc_post }
        }

        pub fn pre_and_rtt(tsc_pre: u64, rtt: u64) -> Self {
            Self {
                tsc_pre,
                tsc_post: tsc_pre + rtt,
            }
        }
    }
}
