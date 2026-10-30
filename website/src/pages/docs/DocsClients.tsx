import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import TerminalWindow from '../../components/TerminalWindow';
import { siteConfig } from '../../data/site';

export default function DocsClients() {
  return (
    <DocsLayout
      title="Clients"
      description="Read bounded timestamps from the daemon using the Rust crate or the C/FFI library. Both require the clockbound daemon to be running."
    >
      <p>
        A ClockBound client reads the daemon&apos;s shared-memory segment and
        returns the current time as an <code>earliest</code>/<code>latest</code>{' '}
        pair plus a clock status. First-party clients are provided as a Rust
        crate and a C library; any language that can memory-map a file can
        implement the <a href="/docs/protocol">protocol</a> directly.
      </p>

      {/* This section replaces the README's broken link to docs/rust-client.md */}
      <AnchorHeading level={2} id="rust">Rust client</AnchorHeading>
      <p>
        Add the <code>clock-bound</code> crate to your{' '}
        <code>Cargo.toml</code>:
      </p>
      <TerminalWindow
        title="Cargo.toml"
        code={`[dependencies]
clock-bound = "${siteConfig.version}"`}
      />
      <p className="mt-4">Then use it in your application:</p>
      <TerminalWindow
        title="main.rs"
        code={`use clock_bound::{Client, ClockStatus, Error, TimeWindow};

fn main() -> Result<(), Error> {
    // Opens the daemon's shared-memory segment.
    let mut client = Client::new()?;

    // Read the current bounded time.
    let window: TimeWindow = client.now()?;

    // Only trust the time window when the clock is synchronized or free running.
    match window.clock_status {
        ClockStatus::Synchronized | ClockStatus::FreeRunning => {
            println!("Time window: {:?} to {:?}", window.earliest, window.latest);
        }
        status => eprintln!("Time window not trustworthy, clock status: {status:?}"),
    }

    Ok(())
}`}
      />
      <p className="mt-4">
        <code>now()</code> returns a <code>TimeWindow</code>: the{' '}
        <code>earliest</code> and <code>latest</code> timestamps that bound the
        current time, along with the clock status (see{' '}
        <a href="/docs/concepts">Concepts</a>). API reference documentation is
        published on docs.rs for the{' '}
        <a
          href={siteConfig.cratesUrl}
          target="_blank"
          rel="noopener noreferrer"
        >
          clock-bound crate
        </a>
        .
      </p>

      <AnchorHeading level={2} id="c">C / FFI client</AnchorHeading>
      <p>
        The <code>clock-bound-ffi</code> crate builds the{' '}
        <code>libclockbound</code> C library. The daemon must be running for the
        library to work.
      </p>

      <AnchorHeading level={3} id="build-library">Build the library</AnchorHeading>
      <TerminalWindow
        title="bash"
        code={`# Build the FFI crate
cargo build --release
# Produces libclockbound.a and libclockbound.so`}
      />

      <AnchorHeading level={3} id="install">Install the header and library</AnchorHeading>
      <TerminalWindow
        title="bash"
        code={`# Copy the header to /usr/include/
sudo cp clock-bound-ffi/include/clockbound.h /usr/include/

# Copy the library artifacts to /usr/lib/
sudo cp target/release/libclockbound.a target/release/libclockbound.so /usr/lib/`}
      />

      <AnchorHeading level={3} id="use-from-c">Use it from C</AnchorHeading>
      <TerminalWindow
        title="clockbound_now.c"
        code={`#include <clockbound.h>

clockbound_now_result now;
clockbound_err cb_err;
clockbound_ctx *ctx = clockbound_open(&cb_err);

clockbound_now(ctx, &now, &cb_err);

// Only trust the time window when the clock is synchronized or free running.
if (now.clock_status == CLOCKBOUND_STA_SYNCHRONIZED ||
    now.clock_status == CLOCKBOUND_STA_FREE_RUNNING) {
    printf("Time window: %ld.%09ld to %ld.%09ld\\n",
           now.earliest.tv_sec, now.earliest.tv_nsec,
           now.latest.tv_sec, now.latest.tv_nsec);
} else {
    fprintf(stderr, "Time window not trustworthy, clock status: %d\\n", now.clock_status);
}`}
      />

      <AnchorHeading level={3} id="compile-and-link">Compile &amp; link</AnchorHeading>
      <p>
        <code>gcc</code> is required. Ensure the loader can find the shared
        library (for example, <code>export LD_LIBRARY_PATH=/usr/lib</code>), then
        compile against <code>-lclockbound</code>:
      </p>
      <TerminalWindow
        title="bash"
        code={`gcc clockbound_now.c -o clockbound_now -I/usr/include -L/usr/lib -lclockbound
./clockbound_now`}
      />
      <p className="mt-4">
        Runnable example programs (<code>clockbound_now</code> and{' '}
        <code>clockbound_loop_forever</code>) are provided in the repository
        under <code>examples/client/c</code>.
      </p>

      <AnchorHeading level={2} id="custom">Custom clients</AnchorHeading>
      <p>
        Clients can be written in any programming language that can read from a
        shared-memory segment backed by a file. See the{' '}
        <a href="/docs/protocol">Protocol reference</a> for the exact
        shared-memory layout and the read-time computation of the clock error
        bound.
      </p>
    </DocsLayout>
  );
}
