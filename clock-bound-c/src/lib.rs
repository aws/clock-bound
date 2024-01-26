// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! A client library to communicate with ClockBoundD.
//! # Usage
//! ClockBoundC requires ClockBoundD to be running to work. See [ClockBoundD documentation](../clock-bound-d/README.md) for installation instructions.
//!
//! For Rust programs built with Cargo, add "clock-bound-c" as a dependency in your Cargo.toml.
//!
//! For example:
//! ```text
//! [dependencies]
//! clock-bound-c = "0.1.0"
//! ```
//!
//! ## Examples
//!
//! Runnable examples exist at [examples](examples) and can be run with Cargo.
//!
//! "/run/clockboundd/clockboundd.sock" is the expected default clockboundd.sock location, but the examples can be run with a
//! different socket location if desired:
//!
//! ```text
//! cargo run --example now /run/clockboundd/clockboundd.sock
//! cargo run --example before /run/clockboundd/clockboundd.sock
//! cargo run --example after /run/clockboundd/clockboundd.sock
//! cargo run --example timing /run/clockboundd/clockboundd.sock
//! ```
//!
//! # Updating README
//!
//! This README is generated via [cargo-readme](https://crates.io/crates/cargo-readme). Updating can be done by running:
//! ```text
//! cargo readme > README.md
//! ```
mod error;

use crate::error::ClockBoundCError;
use byteorder::{ByteOrder, NetworkEndian};
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The default Unix Datagram Socket file that is generated by ClockBoundD.
pub const CLOCKBOUNDD_SOCKET_ADDRESS_PATH: &str = "/run/clockboundd/clockboundd.sock";
/// The prefix of a ClockBoundC socket name. It is appended with a randomized string
/// Ex: clockboundc-G0uv7ULMLNyeLIGKSejG.sock
pub const CLOCKBOUNDC_SOCKET_NAME_PREFIX: &str = "clockboundc";

/// Setting clock frequency to 1ppm to match chrony
pub const FREQUENCY_ERROR: u64 = 1; //1ppm

/// A structure for containing the error bounds returned from ClockBoundD. The values represent
/// the time since the Unix Epoch in nanoseconds.
pub struct Bound {
    /// System time minus the error calculated from chrony in nanoseconds since the Unix Epoch
    pub earliest: u64,
    /// System time plus the error calculated from chrony in nanoseconds since the Unix Epoch
    pub latest: u64,
}

/// A structure for holding the header of the received response.
pub struct ResponseHeader {
    /// The version of the response received from ClockBoundD.
    pub response_version: u8,
    /// The type of the response received from ClockBoundD.
    pub response_type: u8,
    /// A flag representing if Chrony is reporting as unsynchronized.
    pub unsynchronized_flag: bool,
}

/// A structure for holding the response of a now request.
pub struct ResponseNow {
    pub header: ResponseHeader,
    pub bound: Bound,
    /// The timestamp, represented as nanoseconds since the Unix Epoch, bounded against.
    pub timestamp: u64,
}

/// A structure for holding the response of a before request.
pub struct ResponseBefore {
    pub header: ResponseHeader,
    /// A boolean indicating if the requested time is before the current error bounds or not.
    pub before: bool,
}

/// A structure for holding the response of an after request.
pub struct ResponseAfter {
    pub header: ResponseHeader,
    /// A boolean indicating if the requested time is after the current error bounds or not.
    pub after: bool,
}

/// A structure for holding the response of a timing request.
#[derive(Debug)]
pub struct TimingResult {
    /// Callback began executing no earlier than this time
    pub earliest_start: SystemTime,
    /// Callback finished executing no later than this time
    pub latest_finish: SystemTime,
    /// No less than this amount of time elapsed from when timing() was called
    /// to when it returned.
    pub min_execution_time: Duration,
    /// No more than this amount of time elapsed when the callback was invoked
    /// to when it returned.
    pub max_execution_time: Duration,
}

/// A structure for holding a client to communicate with ClockBoundD.
pub struct ClockBoundClient {
    /// A ClockBoundClient must have a socket to communicate with ClockBoundD.
    socket: UnixDatagram,
}

impl ClockBoundClient {
    /// Create a new ClockBoundClient using the default clockboundd.sock path at
    /// "/run/clockboundd/clockboundd.sock".
    ///
    /// # Examples
    ///
    /// ```
    /// use clock_bound_c::ClockBoundClient;
    /// let client = match ClockBoundClient::new(){
    ///     Ok(client) => client,
    ///     Err(e) => {
    ///         println!("Couldn't create client: {}", e);
    ///         return
    ///     }
    /// };
    pub fn new() -> Result<ClockBoundClient, ClockBoundCError> {
        ClockBoundClient::new_with_path(std::path::PathBuf::from(CLOCKBOUNDD_SOCKET_ADDRESS_PATH))
    }
    /// Create a new ClockBoundClient using a defined clockboundd.sock path.
    ///
    /// The expected default socket path is at "/run/clockboundd/clockboundd.sock", but if a
    /// different desired location has been set up this allows its usage.
    ///
    /// If using the default location at "/run/clockboundd/clockboundd.sock", use new() instead.
    ///
    /// # Arguments
    ///
    /// * `clock_bound_d_socket` - The path at which the clockboundd.sock lives.
    ///
    /// # Examples
    ///
    /// ```
    /// use clock_bound_c::ClockBoundClient;
    /// let client = match ClockBoundClient::new_with_path(std::path::PathBuf::from("/run/clockboundd/clockboundd.sock")){
    ///     Ok(client) => client,
    ///     Err(e) => {
    ///         println!("Couldn't create client: {}", e);
    ///         return
    ///     }
    /// };
    /// ```
    pub fn new_with_path(
        clock_bound_d_socket: PathBuf,
    ) -> Result<ClockBoundClient, ClockBoundCError> {
        let client_path = get_socket_path();

        // Binding will fail if the socket file already exists. However, since the socket file is
        // uniquely created based on the current time this should not fail.
        let sock = match UnixDatagram::bind(client_path.as_path()) {
            Ok(sock) => sock,
            Err(e) => return Err(ClockBoundCError::BindError(e)),
        };

        let mode = 0o666;
        let permissions = fs::Permissions::from_mode(mode);
        match fs::set_permissions(client_path, permissions) {
            Err(e) => return Err(ClockBoundCError::SetPermissionsError(e)),
            _ => {}
        }

        match sock.connect(clock_bound_d_socket.as_path()) {
            Err(e) => return Err(ClockBoundCError::ConnectError(e)),
            _ => {}
        }

        Ok(ClockBoundClient { socket: sock })
    }

    /// Returns the bounds of the current system time +/- the error calculated from chrony.
    ///
    /// # Examples
    ///
    /// ```
    /// use clock_bound_c::ClockBoundClient;
    /// let client = match ClockBoundClient::new(){
    ///     Ok(client) => client,
    ///     Err(e) => {
    ///         println!("Couldn't create client: {}", e);
    ///         return
    ///     }
    /// };
    /// let response = match client.now(){
    ///     Ok(response) => response,
    ///     Err(e) => {
    ///         println!("Couldn't complete now request: {}", e);
    ///         return
    ///     }
    /// };
    /// ```
    pub fn now(&self) -> Result<ResponseNow, ClockBoundCError> {
        // Header
        // 1st - Version
        // 2nd - Command Type
        // 3rd, 4th - Reserved
        let mut request: [u8; 4] = [1, 1, 0, 0];

        match self.socket.send(&mut request) {
            Err(e) => return Err(ClockBoundCError::SendMessageError(e)),
            _ => {}
        }
        let mut response: [u8; 20] = [0; 20];
        match self.socket.recv(&mut response) {
            Err(e) => return Err(ClockBoundCError::ReceiveMessageError(e)),
            _ => {}
        }
        let response_version = response[0];
        let response_type = response[1];
        let unsynchronized_flag = response[2] != 0;
        let earliest = NetworkEndian::read_u64(&response[4..12]);
        let latest = NetworkEndian::read_u64(&response[12..20]);
        Ok(ResponseNow {
            header: ResponseHeader {
                response_version,
                response_type,
                unsynchronized_flag,
            },
            bound: Bound { earliest, latest },
            // Since the bounds are the system time +/- the Clock Error Bound, the system time
            // timestamp can be calculated with the below formula.
            timestamp: (latest - ((latest - earliest) / 2)),
        })
    }

    /// Returns true if the provided timestamp is before the earliest error bound.
    /// Otherwise, returns false.
    ///
    /// # Arguments
    ///
    /// * `before_time` - A timestamp, represented as nanoseconds since the Unix Epoch, that is
    /// tested against the earliest error bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use clock_bound_c::ClockBoundClient;
    /// let client = match ClockBoundClient::new(){
    ///     Ok(client) => client,
    ///     Err(e) => {
    ///         println!("Couldn't create client: {}", e);
    ///         return
    ///     }
    /// };
    /// // Using 0 which equates to the Unix Epoch
    /// let response = match client.before(0){
    ///     Ok(response) => response,
    ///     Err(e) => {
    ///         println!("Couldn't complete before request: {}", e);
    ///         return
    ///     }
    /// };
    /// ```
    pub fn before(&self, before_time: u64) -> Result<ResponseBefore, ClockBoundCError> {
        // Header
        // 1st - Version
        // 2nd - Command Type
        // 3rd, 4th - Reserved
        let mut request: [u8; 12] = [1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

        // Body
        NetworkEndian::write_u64(&mut request[4..12], before_time);

        match self.socket.send(&mut request) {
            Err(e) => return Err(ClockBoundCError::SendMessageError(e)),
            _ => {}
        }
        let mut response: [u8; 5] = [0; 5];
        match self.socket.recv(&mut response) {
            Err(e) => return Err(ClockBoundCError::ReceiveMessageError(e)),
            _ => {}
        }
        let response_version = response[0];
        let response_type = response[1];
        let unsynchronized_flag = response[2] != 0;
        let before = response[4] != 0;
        Ok(ResponseBefore {
            header: ResponseHeader {
                response_version,
                response_type,
                unsynchronized_flag,
            },
            before,
        })
    }

    /// Returns true if the provided timestamp is after the latest error bound.
    /// Otherwise, returns false.
    ///
    /// # Arguments
    ///
    /// * `after_time` - A timestamp, represented as nanoseconds since the Unix Epoch, that is
    /// tested against the latest error bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use clock_bound_c::ClockBoundClient;
    /// let client = match ClockBoundClient::new(){
    ///     Ok(client) => client,
    ///     Err(e) => {
    ///         println!("Couldn't create client: {}", e);
    ///         return
    ///     }
    /// };
    /// // Using 0 which equates to the Unix Epoch
    /// let response = match client.after(0){
    ///     Ok(response) => response,
    ///     Err(e) => {
    ///         println!("Couldn't complete after request: {}", e);
    ///         return
    ///     }
    /// };
    /// ```
    pub fn after(&self, after_time: u64) -> Result<ResponseAfter, ClockBoundCError> {
        // Header
        // 1st - Version
        // 2nd - Command Type
        // 3rd, 4th - Reserved
        let mut request: [u8; 12] = [1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

        // Body
        NetworkEndian::write_u64(&mut request[4..12], after_time);

        match self.socket.send(&mut request) {
            Err(e) => return Err(ClockBoundCError::SendMessageError(e)),
            _ => {}
        }
        let mut response: [u8; 5] = [0; 5];
        match self.socket.recv(&mut response) {
            Err(e) => return Err(ClockBoundCError::ReceiveMessageError(e)),
            _ => {}
        }
        let response_version = response[0];
        let response_type = response[1];
        let unsynchronized_flag = response[2] != 0;
        let after = response[4] != 0;
        Ok(ResponseAfter {
            header: ResponseHeader {
                response_version,
                response_type,
                unsynchronized_flag,
            },
            after,
        })
    }

    ///Execute `f` and return bounds on execution time
    pub fn timing<A, F>(&self, f: F) -> Result<(TimingResult, A), (ClockBoundCError, Result<A, F>)>
    where
        F: FnOnce() -> A,
    {
        // Get the first timestamps

        // Header
        // 1st - Version
        // 2nd - Command Type
        // 3rd, 4th - Reserved
        let mut request: [u8; 4] = [1, 1, 0, 0];

        match self.socket.send(&mut request) {
            Err(e) => return Err((ClockBoundCError::SendMessageError(e), Err(f))),
            _ => {}
        }
        let mut response: [u8; 20] = [0; 20];
        match self.socket.recv(&mut response) {
            Err(e) => return Err((ClockBoundCError::ReceiveMessageError(e), Err(f))),
            _ => {}
        }
        let earliest_start = NetworkEndian::read_u64(&response[4..12]);
        let latest_start = NetworkEndian::read_u64(&response[12..20]);

        // Execute the provided function, f
        let callback = f();

        // Get the second timestamps
        let mut request: [u8; 4] = [1, 1, 0, 0];
        match self.socket.send(&mut request) {
            Err(e) => return Err((ClockBoundCError::SendMessageError(e), Ok(callback))),
            _ => {}
        }
        let mut response: [u8; 20] = [0; 20];
        match self.socket.recv(&mut response) {
            Err(e) => return Err((ClockBoundCError::ReceiveMessageError(e), Ok(callback))),
            _ => {}
        }
        let earliest_finish = NetworkEndian::read_u64(&response[4..12]);
        let latest_finish = NetworkEndian::read_u64(&response[12..20]);

        // Calculate midpoints of start and finish
        let start_midpoint = (earliest_start + latest_start) / 2;
        let end_midpoint = (earliest_finish + latest_finish) / 2;

        // Convert to SystemTime
        let earliest_start = UNIX_EPOCH + Duration::from_nanos(earliest_start);
        let latest_finish = UNIX_EPOCH + Duration::from_nanos(latest_finish);

        // Calculates duration between the two midpoints
        let execution_time = end_midpoint - start_midpoint;
        let error_rate = (execution_time * FREQUENCY_ERROR) / 1_000_000 +
            //Ugly way of saying .div_ceil() until it stabilizes
            if (execution_time * FREQUENCY_ERROR) % 1_000_000 == 0 { 0 } else { 1 };

        let min_execution_time = Duration::from_nanos(execution_time - error_rate);
        let max_execution_time = Duration::from_nanos(execution_time + error_rate);

        Ok((
            TimingResult {
                earliest_start,
                latest_finish,
                min_execution_time,
                max_execution_time,
            },
            callback,
        ))
    }
}

impl Drop for ClockBoundClient {
    /// Remove the client socket file when a ClockBoundClient is dropped.
    fn drop(&mut self) {
        if let Ok(addr) = self.socket.local_addr() {
            if let Some(path) = addr.as_pathname() {
                let _ = self.socket.shutdown(std::net::Shutdown::Both);
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

struct MonitorCommand {
    alarm: Duration,
    hysteresis: Duration,
    callback: Box<dyn FnMut(Duration) -> bool + Send>,
}

/// Continuously monitor the health of the system clock
///
/// Instantiating `ClockBoundMonitor` spawns a background thread which will poll
/// the ClockBound daemon once per second and call a user-supplied callback when
/// the clock error bound rises above or below provided thresholds.
///
/// Dropping a `ClockBoundMonitor` or calling its `shutdown()` method will cause all
/// further notifications to cease.
#[derive(Debug)]
pub struct ClockBoundMonitor {
    handle: JoinHandle<()>,
    sender: mpsc::Sender<MonitorCommand>,
}

impl ClockBoundMonitor {
    /// Launches a clock bound monitor which will poll the ClockBound daemon using the provided client.
    pub fn new(client: ClockBoundClient) -> ClockBoundMonitor {
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(|| {
            ClockBoundMonitor::run(client, receiver);
        });

        ClockBoundMonitor { handle, sender }
    }

    fn run(client: ClockBoundClient, receiver: mpsc::Receiver<MonitorCommand>) {
        let mut old_bound = Duration::ZERO;
        let mut active_commands = Vec::new();

        loop {
            let bound = client
                .now()
                .map(|response| {
                    Duration::from_nanos((response.bound.latest - response.bound.earliest) >> 1)
                })
                .unwrap_or(Duration::MAX);

            loop {
                match receiver.try_recv() {
                    Ok(mut cmd) => {
                        // We want the subscriber to get an immediate
                        // notification if it's already in an alarm state. We
                        // check here that cmd.alarm is below *both* the old and
                        // new bounds, because if it's below new one but not the
                        // old one, that will already be handled below.
                        if cmd.alarm <= bound && cmd.alarm <= old_bound {
                            if cmd.callback.as_mut()(bound) {
                                active_commands.push(cmd);
                            }
                        } else {
                            active_commands.push(cmd);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }

            let mut new_active_commands = Vec::with_capacity(active_commands.len());
            for mut cmd in active_commands.into_iter() {
                if (old_bound < cmd.alarm && cmd.alarm <= bound)
                    || (bound < cmd.hysteresis && cmd.hysteresis <= old_bound)
                {
                    if cmd.callback.as_mut()(bound) {
                        new_active_commands.push(cmd);
                    }
                } else {
                    new_active_commands.push(cmd);
                }
            }

            old_bound = bound;
            active_commands = new_active_commands;

            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// Causes the monitor to call `callback` with edge-triggered notifications
    /// that the clock error bound has crossed above `alarm` or below
    /// `hysteresis`. An immediate notification will be sent if the CEB is
    /// initially above `alarm`. Communication failures with the ClockBound
    /// daemon are signaled as if the error bound had become `Duration::MAX`.
    ///  If the callback returns `false`, it will be unsubscribed and never
    /// called again.
    pub fn subscribe<F: FnMut(Duration) -> bool + Send + 'static>(
        &self,
        alarm: Duration,
        hysteresis: Duration,
        callback: F,
    ) {
        let cmd = MonitorCommand {
            alarm,
            hysteresis,
            callback: Box::new(callback),
        };

        self.sender
            .send(cmd)
            .expect("ClockBoundMonitor mpsc channel unexpectedly closed");
    }

    /// Cleanly shuts down the monitoring thread.
    pub fn shutdown(self) -> std::thread::Result<()> {
        std::mem::drop(self.sender);
        self.handle.join()
    }
}

/// Create a unique client socket file in the system's temp directory
///
/// The socket name will have clockboundc as a prefix, followed by a random string of 20
/// alphanumeric characters.
/// Ex: clockboundc-G0uv7ULMLNyeLIGKSejG.sock
fn get_socket_path() -> PathBuf {
    let dir = std::env::temp_dir();
    let mut rng = thread_rng();
    let random_str: String = (&mut rng)
        .sample_iter(Alphanumeric)
        .take(20)
        .map(char::from)
        .collect();

    let client_path_buf =
        dir.join(CLOCKBOUNDC_SOCKET_NAME_PREFIX.to_owned() + "-" + &*random_str + ".sock");
    return client_path_buf;
}
