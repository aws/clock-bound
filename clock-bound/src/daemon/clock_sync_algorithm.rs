//! Feed forward clock sync algorithm

mod selector;
pub use selector::{Selector, SyncParameters};
pub use source::SourceInfo;

pub mod ff;

mod ring_buffer;

use std::{net::SocketAddr, sync::Arc};

pub use ring_buffer::RingBuffer;

use crate::daemon::{
    clock_parameters::ClockParameters, event, logging, receiver_stream::RoutableEvent,
    selected_clock::SelectedClockSource,
};

pub mod source;

/// ClockBound's Clock Sync Algorithm
///
/// The ClockBound clock sync algorithm's role is to consume, transform, and relay input
/// clock sources to answer the singular question:
/// > What time is it?
///
/// The [`ClockSyncAlgorithm`] is a [sans-io](https://sans-io.readthedocs.io/) component
/// that feeds on time synchronization events, and outputs the singular best estimate for
/// the time, TSC frequency, and their associated errors.
///
/// # Usage
/// TODO
#[derive(Debug, Clone, bon::Builder)]
pub struct ClockSyncAlgorithm {
    /// The Amazon Time Sync reference clock's ff algorithm
    amazon_time_sync: Option<source::AmazonTimeSync>,
    /// A vector of ff algorithms for ntp source reference clocks.
    ///
    /// This includes both standalone NTP sources and pool-resolved sources.
    /// Pool sources are distinguished by having `pool_domain` set on the [`source::NtpSource`].
    #[builder(default)]
    ntp_sources: Vec<source::NtpSource>,
    /// The PHC device
    ///
    /// Optional since not every instance supports this
    phc: Option<source::Phc>,
    /// Shared reference to the current selected clock source
    selected_clock: Arc<SelectedClockSource>,
    /// Selector. Chooses the best clock source
    selector: Selector,
    /// Monotonically increasing sequence number for ffevents log entries.
    ///
    /// Assigned to the `init` event (starting at zero) and incremented by one
    /// for every subsequent `feed` or `disruption` event.
    #[builder(default)]
    seq: u64,
}

impl ClockSyncAlgorithm {
    /// Set the Amazon Time Sync source used by the clock sync algorithm
    ///
    /// # Panics
    ///
    /// Panics if an Amazon Time Sync source has already been set.
    pub fn set_amazon_time_sync(&mut self, source: source::AmazonTimeSync) {
        assert!(self.amazon_time_sync.is_none());
        self.amazon_time_sync = Some(source);
    }

    /// Amazon Time Sync getter
    pub fn amazon_time_sync(&self) -> Option<&source::AmazonTimeSync> {
        self.amazon_time_sync.as_ref()
    }

    /// Add a new NTP source to the clock sync algorithm
    ///
    /// # Panics
    ///
    /// Panics if adding a source that already exists
    pub fn add_ntp_source(&mut self, source: source::NtpSource) {
        assert!(
            self.ntp_sources
                .iter()
                .find(|s| s.socket_address() == source.socket_address())
                .is_none(),
            "duplicate addr"
        );
        self.ntp_sources.push(source);
    }

    /// Remove an NTP source
    ///
    /// # Panics
    ///
    /// Panics if the source does not exist
    pub fn remove_ntp_source(&mut self, addr: &SocketAddr) {
        let removed_index = self
            .ntp_sources
            .iter()
            .position(|s| s.socket_address() == *addr)
            .unwrap();

        self.ntp_sources.swap_remove(removed_index);
    }

    /// NTP sources getter
    pub fn ntp_sources(&self) -> &[source::NtpSource] {
        &self.ntp_sources
    }

    /// Set the PHC source used by the clock sync algorithm
    ///
    /// # Panics
    ///
    /// Panics if a PHC source has already been set.
    pub fn set_phc(&mut self, phc: source::Phc) {
        assert!(self.phc.is_none());
        self.phc = Some(phc);
    }

    /// PHC getter
    pub fn phc(&self) -> Option<&source::Phc> {
        self.phc.as_ref()
    }

    /// Logs into the reproducibility logs that the app has started
    ///
    /// Can be used to break-up application restarts when scanning logs
    pub fn init_repro(&mut self) {
        logging::ffevents::log_init(self.next_seq());
    }

    /// Feed the clock sync algorithm with a time synchronization event
    pub fn feed(&mut self, routable_event: RoutableEvent) -> Option<&SyncParameters> {
        #[cfg(not(test))]
        {
            use crate::daemon::event::TscRtt;
            let Some(system_clock) = routable_event.system_clock() else {
                return self.feed_repro(routable_event);
            };
            let system = system_clock.system_time;
            let system_tsc = system_clock.tsc;
            let tsc_rtt = routable_event.rtt();
            let retval = self.feed_repro(routable_event);
            if let Some(new_source_params) = &retval {
                let new_params = &new_source_params.clock_parameters;
                let system_clock_tsc_age = system_tsc - new_params.tsc_count;
                let system_clock_age = system_clock_tsc_age * new_params.period;

                let comparable_time = new_params.time + system_clock_age;
                let offset_from_system_clock = comparable_time - system;
                let ntp_rtt = tsc_rtt * new_params.period;

                tracing::info!(
                    ?system_clock_tsc_age,
                    ?offset_from_system_clock,
                    ?ntp_rtt,
                    ?tsc_rtt
                );
            }
            retval
        }
        #[cfg(test)]
        {
            self.feed_repro(routable_event)
        }
    }

    /// Get the current best source params (clock parameters + source info)
    pub fn current_source_params(&self) -> Option<&SyncParameters> {
        self.selector.current()
    }

    /// Return the current ffevents sequence number, then increment it.
    ///
    /// Used to number every ffevents log entry (`init`/`feed`/`disruption`)
    /// with a monotonically increasing sequence number, starting at zero.
    fn next_seq(&mut self) -> u64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }

    /// Convenience function to allow for easy instrumenting
    fn feed_inner(&mut self, routable_event: RoutableEvent) -> Option<&SyncParameters> {
        // First route the event to the correct inner source
        let alg_output = match routable_event {
            RoutableEvent::AmazonTimeSync(event) => {
                let amazon_time_sync = self.amazon_time_sync.as_mut()?;
                Self::feed_amazon_time_sync(amazon_time_sync, event)
            }
            RoutableEvent::NtpSource(sender_address, event) => {
                Self::feed_ntp_source(&mut self.ntp_sources, sender_address, event)
            }
            RoutableEvent::Phc(device_path, event) => {
                // unwrap: feeding a phc event without PHC built is a bug
                let phc = self.phc.as_mut().unwrap();
                Self::feed_phc(phc, device_path, event)
            }
        };
        let (clock_parameters, source_info) = alg_output?;

        let output = self.selector.update(clock_parameters, source_info.clone());
        if output.is_some() {
            Self::update_selected_clock(&self.selected_clock, &source_info);
        }

        output
    }

    // wrapper around feed_inner that emits reproducibility
    #[expect(
        clippy::needless_pass_by_value,
        reason = "serializing event before logging is unergonomic"
    )]
    fn feed_repro(&mut self, routable_event: RoutableEvent) -> Option<&SyncParameters> {
        let seq = self.next_seq();
        let output = self.feed_inner(routable_event.clone());
        logging::ffevents::log_feed(&routable_event, output.map(|sp| &sp.clock_parameters), seq);
        output
    }

    /// Feed event into the Amazon Time Sync source
    fn feed_amazon_time_sync(
        amazon_time_sync: &mut source::AmazonTimeSync,
        event: event::Ntp,
    ) -> Option<(&ClockParameters, SourceInfo)> {
        // associated method to help borrow checker
        let stratum = event.data().stratum;
        let address = SocketAddr::from(amazon_time_sync.source_address());

        amazon_time_sync
            .feed(event)
            .map(|params| (params, SourceInfo::AmazonTimeSync(address, stratum)))
    }

    /// Feed event into an ntp source (standalone or pool-resolved)
    ///
    /// Matches by socket address against all `ntp_sources`.
    fn feed_ntp_source(
        ntp_sources: &mut [source::NtpSource],
        sender_address: SocketAddr,
        event: event::Ntp,
    ) -> Option<(&ClockParameters, SourceInfo)> {
        // associated method to help borrow checker
        let stratum = event.data().stratum;
        ntp_sources
            .iter_mut()
            .find(|source| source.socket_address() == sender_address)
            .and_then(|source| source.feed(event))
            .map(|params| (params, SourceInfo::NtpSource(sender_address, stratum)))
    }

    /// Feed event into the phc
    fn feed_phc(
        phc: &mut source::Phc,
        device_path: source::DevicePath,
        event: event::Phc,
    ) -> Option<(&ClockParameters, SourceInfo)> {
        phc.feed(event)
            .map(|params| (params, SourceInfo::Phc(device_path)))
    }

    fn update_selected_clock(selected_clock: &Arc<SelectedClockSource>, source_info: &SourceInfo) {
        // associated method to help borrow checker
        match source_info {
            SourceInfo::AmazonTimeSync(address, stratum)
            | SourceInfo::NtpSource(address, stratum) => {
                selected_clock.set_to_server(address.ip(), *stratum);
            }
            SourceInfo::Phc(_) => selected_clock.set_to_phc(),
        }
    }

    /// Handle a clock disruption event
    ///
    /// Call this function after the system detects a VMClock disruption event.
    ///
    /// It will go through and clear the state (like startup).
    pub fn handle_disruption(&mut self) {
        // Use the destructure pattern to get a mutable reference to each item.
        //
        // This makes it a compilation error if we add a new field this Self without handling it here
        let Self {
            amazon_time_sync,
            ntp_sources,
            phc,
            selected_clock,
            selector,
            seq,
        } = self;

        selected_clock.set_to_none();
        if let Some(amazon_time_sync) = amazon_time_sync {
            amazon_time_sync.handle_disruption();
        }
        for source in ntp_sources {
            source.handle_disruption();
        }
        if let Some(phc) = phc {
            phc.handle_disruption();
        }

        selector.handle_disruption();
        tracing::info!("Handled clock disruption event");
        let current_seq = *seq;
        *seq += 1;
        logging::ffevents::log_disruption(current_seq);
    }
}

#[cfg(test)]
mod tests {
    use core::str;
    use std::{net::Ipv4Addr, str::FromStr};

    use rstest::rstest;

    use crate::daemon::{
        event::Stratum,
        selected_clock::ClockSource,
        time::{Duration, Instant, TscCount, tsc::Skew},
    };

    use super::*;

    #[test]
    #[tracing_test::traced_test]
    fn feed_serializes_events() {
        // Most logs are permeable to change. Make sure that we log a json event.

        let event = event::Ntp::builder()
            .counter_pre(TscCount::new(500))
            .counter_post(TscCount::new(1000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(2),
                root_delay: Duration::from_micros(50),
                root_dispersion: Duration::from_millis(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        let event = RoutableEvent::AmazonTimeSync(event);

        let mut csa = ClockSyncAlgorithm::builder()
            .amazon_time_sync(source::AmazonTimeSync::new(Skew::from_ppm(15.0)))
            .ntp_sources(vec![])
            .selected_clock(Arc::new(SelectedClockSource::default()))
            .selector(Selector::new(Skew::from_ppm(15.0)))
            .build();

        let clock_parameters = csa.feed(event.clone());
        assert!(clock_parameters.is_none());

        use crate::daemon::logging::ffevents::types::{ClockParametersLog, RoutableEventLog};
        let serialized_event = serde_json::to_string(&RoutableEventLog::from(&event)).unwrap();
        let serialized_output = serde_json::to_string(
            &clock_parameters.map(|sp| ClockParametersLog::from(&sp.clock_parameters)),
        )
        .unwrap();

        // tracing escapes quotes
        let serialized_event = serialized_event.replace("\"", r#"\""#);
        let serialized_output = serialized_output.replace("\"", r#"\""#);

        assert!(logs_contain(&serialized_event));
        assert!(logs_contain(&serialized_output));
    }

    #[rstest]
    #[case(SourceInfo::AmazonTimeSync("169.254.169.123:123".parse().unwrap(), Stratum::TWO), ClockSource::Server(Ipv4Addr::from_str("169.254.169.123").unwrap().into()), Stratum::TWO)]
    #[case(SourceInfo::NtpSource("169.254.169.101:123".parse().unwrap(), Stratum::ONE), ClockSource::Server(Ipv4Addr::from_str("169.254.169.101").unwrap().into()), Stratum::ONE)]
    #[case(SourceInfo::NtpSource("[2001:db8::1:1234]:123".parse().unwrap(), Stratum::TWO), ClockSource::Server(Ipv4Addr::from_str("199.132.19.175").unwrap().into()), Stratum::TWO)]
    #[case(SourceInfo::Phc("/dev/ptp0".into()), ClockSource::Phc, Stratum::Unspecified)]
    fn update_selected_clock(
        #[case] source_info: SourceInfo,
        #[case] expected_clock_source: ClockSource,
        #[case] expected_stratum: Stratum,
    ) {
        let selected_clock_source = Arc::new(SelectedClockSource::default());
        ClockSyncAlgorithm::update_selected_clock(&selected_clock_source, &source_info);
        let (clock_source, stratum) = selected_clock_source.get();
        assert_eq!(clock_source, expected_clock_source);
        assert_eq!(stratum, expected_stratum);
    }

    fn make_ntp_event(pre: i64, post: i64) -> event::Ntp {
        event::Ntp::builder()
            .counter_pre(TscCount::new(pre))
            .counter_post(TscCount::new(post))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(2),
                root_delay: Duration::from_micros(50),
                root_dispersion: Duration::from_millis(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap()
    }

    fn make_csa() -> ClockSyncAlgorithm {
        ClockSyncAlgorithm::builder()
            .selected_clock(Arc::new(SelectedClockSource::default()))
            .selector(Selector::new(Skew::from_ppm(15.0)))
            .build()
    }

    #[test]
    fn add_pool_source_and_feed() {
        let mut csa = make_csa();
        let pool_domain = "pool.ntp.org".to_string();
        let addr: SocketAddr = "192.0.2.1:123".parse().unwrap();

        csa.add_ntp_source(source::NtpSource::new_with_pool(
            addr,
            pool_domain,
            Skew::from_ppm(15.0),
        ));

        let event = RoutableEvent::NtpSource(addr, make_ntp_event(500, 1000));
        // First feed won't produce parameters (fresh ff algorithm)
        let result = csa.feed(event);
        assert!(result.is_none());
    }

    #[test]
    fn remove_pool_source_removes_correct_source() {
        let mut csa = make_csa();
        let pool_domain = "pool.ntp.org".to_string();
        let addr: SocketAddr = "192.0.2.1:123".parse().unwrap();

        csa.add_ntp_source(source::NtpSource::new_with_pool(
            addr,
            pool_domain,
            Skew::from_ppm(15.0),
        ));
        assert_eq!(csa.ntp_sources().len(), 1);

        csa.remove_ntp_source(&addr);
        assert_eq!(csa.ntp_sources().len(), 0);
    }
}
