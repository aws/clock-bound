// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only

//! Unix signal handler registration.
//!
//! Use the nix crate to register signal callbacks, while keeping any specific notion of libc
//! within this module only. The callbacks are registered into a HashMap and looked up when a
//! signal is received.

use lazy_static::lazy_static;
use libc;
use nix::sys::signal;
use std::collections::HashMap;
use std::io::Result;
use std::sync::Mutex;
use tracing::{error, info};

/// Defines the types of callback that can be registered with the signal handler
type Callback = fn() -> ();

/// Tiny structure to maintain the association of callbacks registered with signals.
///
/// The internal representation is a hashmap of signal number and callbacks.
struct SignalHandler {
    handlers: HashMap<signal::Signal, Callback>,
}

impl SignalHandler {
    /// A new empty SignalHandler structure.
    fn new() -> SignalHandler {
        SignalHandler {
            handlers: HashMap::new(),
        }
    }

    /// Get the callback associated with a signal number.
    ///
    /// Returns the callback wrapped in an Option. Returns None if no callback has been registered
    /// with the given signal.
    fn get_callback(&self, sig: signal::Signal) -> Option<&Callback> {
        self.handlers.get(&sig)
    }

    /// Set / Overwrite callback for a given signal
    ///
    /// Silently ignore the return value of inserting a new callback over an existing one in the
    /// HashMap. Last callback registered wins.
    fn add_callback(&mut self, sig: signal::Signal, callback: Callback) {
        self.handlers.insert(sig, callback);
    }
}

lazy_static! {
    /// Global SignalHandler structure, instantiated on first access.
    ///
    /// Signal handlers have a predefined signature, easier to provide a static variable to lookup the
    /// callbacks to run.
    static ref SIGNAL_HANDLERS: Mutex<SignalHandler> = Mutex::new(SignalHandler::new());
}

/// Main signal handler function.
///
/// This function is the one and unique signal handler, looking up and running registered callbacks.
/// This level of indirection helps hide libc specific details away. Potential drawback is that
/// assessing complexity of the callabck is less obvious.
extern "C" fn main_signal_handler(signum: libc::c_int) {
    // Although unlikely, there is always the risk the registration function holds the lock while
    // the main thread is interrupted by a signal. Do not want to deadlock in interrupted context.
    // Try the lock, and bail out if it cannot be acquired.
    let handlers = match SIGNAL_HANDLERS.try_lock() {
        Ok(handlers) => handlers,
        Err(_) => return, // TODO: log an error?
    };

    if let Ok(sig) = signal::Signal::try_from(signum) {
        if let Some(cb) = handlers.get_callback(sig) {
            cb()
        }
    }
}

/// Enable UNIX signal via sigaction.
///
/// Gathers all libc crate and C types unsafe code here.
fn enable_signal(sig: signal::Signal) -> Result<()> {
    // Always register the main signal handler
    let handler = signal::SigHandler::Handler(main_signal_handler);
    let mask = signal::SigSet::empty();
    let mut flags = signal::SaFlags::empty();
    flags.insert(signal::SaFlags::SA_RESTART);
    flags.insert(signal::SaFlags::SA_SIGINFO);
    flags.insert(signal::SaFlags::SA_NOCLDSTOP);

    let sig_action = signal::SigAction::new(handler, flags, mask);

    let result = unsafe { signal::sigaction(sig, &sig_action) };

    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(std::io::Error::last_os_error()),
    }
}

/// Enable signal and register associated callback.
///
/// Signal handling is done through indirection, hidden from the caller. The master signal handler
/// is always registered to handle the signal. It is then charged with looking up and running the
/// callback provided.
///
/// Should be called on the main thread.
///
/// # Examples
///
/// ```rust
/// use nix::sys::signal;
/// use clock_bound_d::signal::register_signal_callback;
///
/// fn on_sighup() {
///   println!("Got HUP'ed!!");
/// }
///
/// register_signal_callback(signal::SIGHUP, on_sighup);
///
/// ```
pub fn register_signal_callback(sig: signal::Signal, callback: Callback) -> Result<()> {
    // All signals are managed and handled on the main thread. It is safe to lock the mutex and
    // block until acquired. The signal handler may hold the Mutex lock, but releases it once
    // signal handling and main execution resumes.
    let mut handlers = SIGNAL_HANDLERS.lock().unwrap();
    handlers.add_callback(sig, callback);

    // The new callback is registered, the signal can be handled
    match enable_signal(sig) {
        Ok(_) => {
            info!("Registered callback for signal {}", sig);
            Ok(())
        }
        Err(e) => {
            error!("Failed to register callback for signal {}: {}", sig, e);
            Err(e)
        }
    }
}

#[cfg(test)]
mod t_signal {

    use super::*;

    /// Assert that a callaback can be registered and retrieved with the same signal.
    #[test]
    fn test_add_and_get_callback() {
        // Testing side effects is inherently unsafe
        static mut VAL: i32 = 0;
        unsafe {
            let mut handlers = SignalHandler::new();
            VAL = 2;
            fn do_double() {
                unsafe { VAL *= 2 }
            }
            handlers.add_callback(signal::SIGHUP, do_double);
            let cb = handlers.get_callback(signal::SIGHUP).unwrap();
            cb();
            assert_eq!(4, VAL);
        }
    }

    /// Assert that the last callback registered is retrieved and triggered upon multiple
    /// registrations.
    #[test]
    fn test_last_callback_wins() {
        // Testing side effects is inherently unsafe
        static mut VAL: i32 = 2;
        unsafe {
            let mut handlers = SignalHandler::new();
            //VAL = 2;
            fn do_double() {
                unsafe { VAL *= 2 }
            }
            fn do_triple() {
                unsafe { VAL *= 3 }
            }
            fn do_quadruple() {
                unsafe { VAL *= 4 }
            }
            handlers.add_callback(signal::SIGHUP, do_double);
            handlers.add_callback(signal::SIGHUP, do_triple);
            handlers.add_callback(signal::SIGHUP, do_quadruple);
            let cb = handlers.get_callback(signal::SIGHUP).unwrap();
            cb();
            assert_eq!(8, VAL);
        }
    }

    /// Assert that None is returned if no callback is registered for the signal.
    #[test]
    fn test_get_none_on_missing_callbacks() {
        let mut handlers = SignalHandler::new();
        fn do_nothing() {}
        handlers.add_callback(signal::SIGHUP, do_nothing);
        let cb = handlers.get_callback(signal::SIGINT);
        assert_eq!(None, cb);
    }
}
