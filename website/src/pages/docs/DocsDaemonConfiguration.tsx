import { Link } from 'react-router-dom';
import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import TerminalWindow from '../../components/TerminalWindow';
import { siteConfig } from '../../data/site';

const exampleConfig = `[logging]
log_directory = "/var/log/clockbound"     # directory for the log streams below

[logging.synchronization]
enabled = true

[logging.sources]
enabled = true

[logging.ffevents]
enabled = true

# The sources below use documentation-only addresses and names that never
# resolve or respond. Replace them with your own, or remove the blocks.
[[sources.ntp]]                           # a single server, by IPv4
server = "192.0.2.1"

[[sources.ntp]]                           # a single server, by IPv6
server = "2001:db8::123"

[[sources.ntp]]                           # a single server, by hostname
server = "ntp.corp.example"

[[sources.ntp]]                           # a name expected to resolve to several servers
pool = "pool.ntp.example"`;

// A setting's name followed by its type/default facts, matching the
// "#### `key`" + bullet list layout of the source doc.
function Setting({ name, facts }: { name: string; facts: React.ReactNode[] }) {
  return (
    <>
      <p className="mb-2">
        <strong>
          <code>{name}</code>
        </strong>
      </p>
      <ul className="!mt-2">
        {facts.map((fact, i) => (
          <li key={i}>{fact}</li>
        ))}
      </ul>
    </>
  );
}

export default function DocsDaemonConfiguration() {
  return (
    <DocsLayout
      title="Configuration"
      description="Turn on log file streams and add NTP time sources with an optional TOML file."
    >
      <p>
        ClockBound is designed to run with zero configuration. It detects and
        uses the best time sources on the instance on its own. Configuration
        exists for two things: turning on log file streams, and adding NTP time
        sources.
      </p>

      <AnchorHeading level={2} id="quick-start">Quick start</AnchorHeading>
      <p>ClockBound reads a configuration file only when you point it at one:</p>
      <TerminalWindow title="bash" code={`clockbound --configuration /path/to/clockbound.toml`} />
      <p>
        With no <code>--configuration</code> (<code>-c</code>) flag, the daemon
        runs on built-in defaults and auto-detected time sources, logging to
        stdout/stderr only.
      </p>

      <AnchorHeading level={2} id="example">Example configuration</AnchorHeading>
      <p>
        The configuration file format is{' '}
        <a href="https://toml.io/" target="_blank" rel="noopener noreferrer">
          TOML
        </a>
        ; an empty file behaves the same as no file at all. The block below
        illustrates the available settings.
      </p>
      <TerminalWindow title="clockbound.toml" code={exampleConfig} />

      <AnchorHeading level={2} id="logging">
        <code>[logging]</code>
      </AnchorHeading>
      <p>
        Controls ClockBound&apos;s file log streams. For details on the streams
        themselves, see the{' '}
        <Link to="/docs/daemon/logging">ClockBound Logging</Link> documentation.
      </p>
      <p>
        A stream is written only when its section is present with{' '}
        <code>enabled = true</code>.
      </p>
      <p>
        <strong>
          <em>
            Enabled file streams grow without bound unless you rotate them.
            Manage them with retention set to your disk budget and audit needs.
          </em>
        </strong>{' '}
        See the example{' '}
        <a href={`${siteConfig.githubUrl}/blob/main/clock-bound/assets/clockbound.logrotate`} target="_blank" rel="noopener noreferrer">
          logrotate configuration
        </a>
        .
      </p>

      <AnchorHeading level={3} id="logging-log-directory">
        <code>logging.log_directory</code>
      </AnchorHeading>
      <ul>
        <li>Type: string</li>
        <li>Required when any log stream is enabled.</li>
        <li>Must be an absolute path.</li>
      </ul>
      <p>The directory that holds every enabled log file stream.</p>

      <AnchorHeading level={3} id="logging-synchronization">
        <code>[logging.synchronization]</code>
      </AnchorHeading>
      <Setting
        name="logging.synchronization.enabled"
        facts={[
          'Type: boolean',
          <>
            Default: <code>false</code>
          </>,
        ]}
      />
      <p>
        Enables the{' '}
        <Link to="/docs/daemon/logging/synchronization">
          <code>synchronization</code>
        </Link>{' '}
        log stream.
      </p>

      <AnchorHeading level={3} id="logging-sources">
        <code>[logging.sources]</code>
      </AnchorHeading>
      <Setting
        name="logging.sources.enabled"
        facts={[
          'Type: boolean',
          <>
            Default: <code>false</code>
          </>,
        ]}
      />
      <p>
        Enables the{' '}
        <Link to="/docs/daemon/logging/sources">
          <code>sources</code>
        </Link>{' '}
        log stream.
      </p>

      <AnchorHeading level={3} id="logging-ffevents">
        <code>[logging.ffevents]</code>
      </AnchorHeading>
      <Setting
        name="logging.ffevents.enabled"
        facts={[
          'Type: boolean',
          <>
            Default: <code>false</code>
          </>,
        ]}
      />
      <p>
        Enables the{' '}
        <Link to="/docs/daemon/logging/ffevents">
          <code>ffevents</code>
        </Link>{' '}
        log stream.
      </p>

      <AnchorHeading level={2} id="sources">
        <code>[sources]</code>
      </AnchorHeading>
      <p>
        Sources here are added to what ClockBound already uses. Add them when
        you have a specific need, such as an on-premises reference clock or a
        national metrology institute&apos;s pool.
      </p>
      <p>
        You can configure at most{' '}
        <strong>
          5 <code>server</code> entries and 1 <code>pool</code>
        </strong>
        ; exceeding either fails startup. Each source may appear only once: the
        same value listed twice, including the same name under both{' '}
        <code>server</code> and <code>pool</code>, also fails startup.
      </p>

      <AnchorHeading level={3} id="sources-ntp">
        <code>[[sources.ntp]]</code>
      </AnchorHeading>
      <p>
        Each block adds one NTP time source, written as <strong>either</strong>{' '}
        a <code>server</code> <strong>or</strong> a <code>pool</code>.
      </p>
      <Setting
        name="sources.ntp.server"
        facts={['Type: string (hostname or IPv4/IPv6 address)']}
      />
      <p>
        A single time source. If a name resolves to several addresses, only one
        is used; use <code>pool</code> for the set.
      </p>
      <Setting name="sources.ntp.pool" facts={['Type: string (name)']} />
      <p>
        A name expected to resolve to several servers, rather than the single
        source a <code>server</code> entry names. A pool must be a name; an
        IPv4 or IPv6 address is rejected.
      </p>

      <AnchorHeading level={2} id="validation">Validation and startup</AnchorHeading>
      <p>
        The file is read once, at startup. To apply a configuration change,
        restart the daemon.
      </p>
      <p>
        ClockBound validates the whole file before it starts. Any problem — a
        missing or unreadable configuration file, a syntax or type error, an
        unknown or misspelled setting, or an invalid combination — is a hard
        startup failure, where the daemon exits with code <code>2</code> and
        writes the cause to stderr. The rules checked after parsing are:
      </p>
      <ul>
        <li>
          At most 5 <code>server</code> entries and 1 <code>pool</code> entry.
        </li>
        <li>No source value listed more than once.</li>
        <li>
          A <code>pool</code> value is a name, never an IP address.
        </li>
        <li>
          <code>logging.log_directory</code> is an absolute path.
        </li>
        <li>
          <code>logging.log_directory</code> is set when any log stream is
          enabled.
        </li>
      </ul>
      <p>
        A file that parses is checked against all of these rules at once, and
        every violation is reported in the same error.
      </p>
      <p>
        To validate a file without starting the daemon, pass{' '}
        <code>--check-configuration</code> along with it. The daemon reports
        whether the file is valid and exits.
      </p>
      <TerminalWindow
        title="bash"
        code={`clockbound --check-configuration --configuration /path/to/clockbound.toml`}
      />

      <AnchorHeading level={2} id="verbosity">Log verbosity for stdout/stderr</AnchorHeading>
      <p>
        The verbosity of the daemon&apos;s stdout/stderr output is set by a
        command-line flag:
      </p>
      <ul>
        <li>
          <code>-v</code> / <code>--verbose</code> raises verbosity from the
          default <code>info</code> level to <code>debug</code>.
        </li>
      </ul>
      <p>
        File log streams are unaffected by this flag. They have their own{' '}
        <code>[logging.*]</code> toggles and always log their defined content.
      </p>
      <p>Pass the flag when you start the daemon:</p>
      <TerminalWindow
        title="bash"
        code={`clockbound --configuration /path/to/clockbound.toml -v`}
      />
    </DocsLayout>
  );
}
