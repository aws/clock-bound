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
        pub counter_pre: u64,
        pub counter_post: u64,
    }

    impl TscRtt for TestEvent {
        fn counter_post(&self) -> TscCount {
            TscCount::new(self.counter_post as i64)
        }

        fn counter_pre(&self) -> TscCount {
            TscCount::new(self.counter_pre as i64)
        }
    }

    impl TestEvent {
        pub fn new(counter_pre: u64, counter_post: u64) -> Self {
            Self {
                counter_pre,
                counter_post,
            }
        }

        pub fn pre_and_rtt(counter_pre: u64, rtt: u64) -> Self {
            Self {
                counter_pre,
                counter_post: counter_pre + rtt,
            }
        }
    }
}
