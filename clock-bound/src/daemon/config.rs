//! Daemon configuration: parse and validate the ClockBound TOML config.
//!
//! The daemon reads a TOML file once at startup. `serde`/`toml` deserialize the
//! file into [`Config`]; a semantic pass then checks the cross-field and
//! cardinality rules. Errors render with [`std::fmt::Display`] so the parse
//! error's line/column snippet is preserved.

use {
    std::{
        fmt::Write as _,
        net::IpAddr,
        path::{Path, PathBuf},
    },
    thiserror::Error,
};

/// Maximum number of `server` entries a config may set.
const MAX_SERVERS: usize = 5;

/// Maximum number of `pool` entries a config may set.
const MAX_POOLS: usize = 1;

/// The full daemon configuration.
///
/// Every field defaults, so an empty file is a valid config. Unknown fields are
/// rejected at every level.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Log directory and per-stream toggles.
    logging: LoggingConfig,
    /// Additional sources, used alongside the built-in ones.
    ///
    /// Omitted when empty so that serializing a configuration only ever produces
    /// the `[[sources.ntp]]` form the schema documents: an empty vector would
    /// otherwise render as `ntp = []`, which nothing tells a reader to write.
    #[serde(skip_serializing_if = "SourcesConfig::is_empty")]
    sources: SourcesConfig,
}

impl Config {
    /// Take the configuration apart, for handing each half to whatever uses it.
    #[must_use]
    pub fn into_parts(self) -> (LoggingConfig, SourcesConfig) {
        (self.logging, self.sources)
    }
}

/// Logging configuration.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LoggingConfig {
    /// Directory the file streams are written to. Required when any stream is
    /// enabled; must be an absolute path.
    log_directory: Option<PathBuf>,
    /// Human-readable clock synchronization history.
    synchronization: StreamConfig,
    /// Raw sync data for expert replay.
    ffevents: StreamConfig,
}

impl LoggingConfig {
    /// The directory the file streams are written to, if one is configured.
    pub fn log_directory(&self) -> Option<&Path> {
        self.log_directory.as_deref()
    }

    /// The synchronization stream.
    pub fn synchronization(&self) -> &StreamConfig {
        &self.synchronization
    }

    /// The ffevents stream.
    pub fn ffevents(&self) -> &StreamConfig {
        &self.ffevents
    }
}

/// A single log stream's configuration.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StreamConfig {
    /// Whether this stream is written to disk.
    enabled: bool,
}

impl StreamConfig {
    /// Whether this stream is written to disk.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

/// Additional time sources, used alongside the built-in ones.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SourcesConfig {
    /// NTP sources, each exactly one of `server` or `pool`.
    ntp: Vec<NtpSource>,
}

impl SourcesConfig {
    /// The configured NTP sources.
    pub fn ntp(&self) -> &[NtpSource] {
        &self.ntp
    }

    /// Whether no source of any kind is configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ntp.is_empty()
    }
}

/// A single additional NTP source. The TOML key (`server` or `pool`) selects the
/// variant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum NtpSource {
    /// A single server, addressed directly or by a name to resolve.
    Server(Host),
    /// A name expected to resolve to several servers. A pool is a name, so an
    /// address is never one.
    Pool(DomainName),
}

impl NtpSource {
    /// The value as written, for comparison and messages. Deliberately drops
    /// which key carried it, so the same name under `server` and `pool` compares
    /// equal.
    fn value(&self) -> String {
        match self {
            Self::Server(host) => host.to_string(),
            Self::Pool(name) => name.to_string(),
        }
    }
}

/// A name to resolve.
///
/// The criteria a string has to meet to become one, where each comes from, and
/// what it catches:
/// - it is not an address: RFC 1123 section 2.1 holds that "a valid host name can
///   never have the dotted-decimal form #.#.#.#", so a value that parses as an
///   IPv4 address is not a name; an IPv6 literal is caught by the colon rule
///   below;
/// - its final label is not all digits: "at least the highest-level component
///   label will be alphabetic" (RFC 1123 section 2.1) is what keeps a name and an
///   address distinguishable, and it catches a mistyped address here rather than
///   at resolution;
/// - it is not empty and no label is: a label opens with a letter, so the grammar
///   admits neither an empty label nor an empty name (RFC 1035 section 2.3.1);
/// - it has no whitespace left inside it, no control characters, and no colon: the
///   preferred name syntax admits only letters, digits and hyphen in a label (RFC
///   1035 section 2.3.1). Whitespace around the value is trimmed first, while
///   whitespace between labels is not removed. A colon is either a botched
///   IPv6 literal or a port suffix, and control characters may lead to undesired
///   behavior.
///
/// Otherwise deliberately permissive. RFC 1035 section 2.3.1 describes a
/// *preferred* syntax rather than a limit, so the character rejections above are
/// policy it supports, not a rule it imposes. Its grammar also excludes underscores
/// and non-ASCII labels, but both are common in practice, so they are accepted here
/// and left to the resolver.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DomainName(String);

impl TryFrom<String> for DomainName {
    type Error = DomainNameError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl DomainName {
    /// Take `value` as a name, if it meets the criteria above.
    ///
    /// # Errors
    ///
    /// Returns [`DomainNameError`] if the value is an address or cannot be a name.
    pub fn parse(value: String) -> Result<Self, DomainNameError> {
        let name = value.trim();

        if name.parse::<IpAddr>().is_ok() {
            return Err(DomainNameError::IsAddress(value));
        }
        // The final label, past the root dot a fully qualified name may end with:
        // "ntp.corp.example." -> "example", "192.0.2.999" -> "999".
        let final_label = name.trim_end_matches('.').rsplit('.').next().unwrap_or("");
        if name.is_empty()
            || name.chars().any(|c| c.is_whitespace() || c.is_control())
            || name.contains(':')
            || final_label.is_empty()
            || final_label.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(DomainNameError::NotAName(value));
        }
        Ok(Self(name.to_string()))
    }

    /// The name as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a string cannot be a [`DomainName`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainNameError {
    /// The value is an address, which a name never is.
    #[error("{0:?} is an IP address, not a name")]
    IsAddress(String),

    /// The value cannot be a name.
    #[error("{0:?} is not a valid name")]
    NotAName(String),
}

impl std::fmt::Display for DomainName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<DomainName> for String {
    fn from(value: DomainName) -> Self {
        value.0
    }
}

/// How a source is reached: an address used as given, or a name to resolve.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Host {
    /// An address, used as given.
    Ip(IpAddr),
    /// A name, resolved to an address.
    Domain(DomainName),
}

impl Host {
    /// Take `value` as an address if it is one, otherwise as a name.
    ///
    /// # Errors
    ///
    /// Returns [`HostError`] if the value is neither.
    pub fn parse(value: String) -> Result<Self, HostError> {
        if let Ok(ip) = value.trim().parse::<IpAddr>() {
            return Ok(Self::Ip(ip));
        }
        DomainName::parse(value)
            .map(Self::Domain)
            .map_err(|e| match e {
                DomainNameError::IsAddress(value) | DomainNameError::NotAName(value) => {
                    HostError(value)
                }
            })
    }
}

impl TryFrom<String> for Host {
    type Error = HostError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

/// Why a string is neither an address nor a [`DomainName`].
#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0:?} is not an IP address or a valid name")]
pub struct HostError(String);

impl std::fmt::Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ip(ip) => write!(f, "{ip}"),
            Self::Domain(name) => write!(f, "{name}"),
        }
    }
}

/// The bare string a configuration holds, not a variant, so a serialized config
/// parses back. Rendering rather than returning a stored string normalizes an
/// address to its canonical spelling: `2001:0db8::1` comes back as `2001:db8::1`.
impl From<Host> for String {
    fn from(value: Host) -> Self {
        value.to_string()
    }
}

/// An error loading the configuration.
///
/// Render with `Display`, not `Debug`: [`ConfigError::Parse`] wraps
/// [`toml::de::Error`], whose `Display` produces the line/column snippet that
/// `{:?}` drops.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// An explicitly requested config file does not exist.
    #[error("Configuration file not found: {0}")]
    NotFound(PathBuf),

    /// An explicitly requested config file exists but could not be read.
    #[error("Could not read configuration file {path}: {source}")]
    Unreadable {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// The file is not valid TOML or does not match the schema.
    #[error("Invalid configuration syntax: {0}")]
    Parse(#[from] toml::de::Error),

    /// The file parsed but violates one or more semantic rules. The message
    /// lists every violation found.
    #[error("Invalid configuration:\n{0}")]
    Invalid(String),
}

impl Config {
    /// Load and validate the configuration.
    ///
    /// `None` returns the default configuration without reading a file.
    /// `Some(path)` reads, parses, and validates the file at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if an explicitly requested file is missing or
    /// unreadable, is not valid TOML / does not match the schema, or violates a
    /// semantic rule.
    pub fn load(path: Option<PathBuf>) -> Result<Config, ConfigError> {
        let Some(path) = path else {
            return Ok(Config::default());
        };

        let contents = std::fs::read_to_string(&path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ConfigError::NotFound(path)
            } else {
                ConfigError::Unreadable { path, source }
            }
        })?;

        let config: Config = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    /// Check the semantic rules, collecting every violation into one message.
    fn validate(&self) -> Result<(), ConfigError> {
        let mut violations: Vec<String> = Vec::new();

        let servers = self
            .sources
            .ntp
            .iter()
            .filter(|s| matches!(s, NtpSource::Server(_)))
            .count();
        let pools = self
            .sources
            .ntp
            .iter()
            .filter(|s| matches!(s, NtpSource::Pool(_)))
            .count();
        if servers > MAX_SERVERS {
            violations.push(format!(
                "at most {MAX_SERVERS} `server` entries are supported, found {servers}"
            ));
        }
        if pools > MAX_POOLS {
            violations.push(format!(
                "at most {MAX_POOLS} `pool` entry is supported, found {pools}"
            ));
        }

        // Each value was already parsed into an address or a name, so all that is
        // left is to catch the same source named twice.
        let mut seen: Vec<String> = Vec::new();
        for source in &self.sources.ntp {
            let value = source.value();
            if seen.contains(&value) {
                violations.push(format!("duplicate source value: {value:?}"));
            } else {
                seen.push(value);
            }
        }

        // Only supporting absolute paths to avoid foot guns
        if let Some(dir) = &self.logging.log_directory
            && !dir.is_absolute()
        {
            violations.push(format!(
                "`log_directory` must be an absolute path: {}",
                dir.display()
            ));
        }

        // The reverse is permitted: a directory with no stream enabled writes
        // nothing, which is what the toggles say.
        let any_stream_enabled =
            self.logging.synchronization.enabled || self.logging.ffevents.enabled;
        if any_stream_enabled && self.logging.log_directory.is_none() {
            violations
                .push("`log_directory` is required when any log stream is enabled".to_string());
        }

        if violations.is_empty() {
            return Ok(());
        }

        let mut message = String::new();
        for violation in &violations {
            let _ = writeln!(message, "  - {violation}");
        }
        Err(ConfigError::Invalid(message.trim_end().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    /// The example config carried in the repository, which has to stay parseable.
    const EXAMPLE_CONFIG: &str = include_str!("../../assets/example-config.toml");

    /// Parse then validate from a string.
    fn parse(s: &str) -> Result<Config, ConfigError> {
        let config: Config = toml::from_str(s)?;
        config.validate()?;
        Ok(config)
    }

    /// A `server` source. Panics unless `value` is an address or a valid name.
    fn server(value: &str) -> NtpSource {
        NtpSource::Server(Host::parse(value.to_string()).unwrap())
    }

    /// A `pool` source. Panics unless `value` is a valid name.
    fn pool(value: &str) -> NtpSource {
        NtpSource::Pool(DomainName::parse(value.to_string()).unwrap())
    }

    /// Whole configurations that must be accepted.
    #[test]
    fn configurations_accepted() {
        for (name, contents) in [
            ("empty", ""),
            (
                "logging with every stream",
                indoc! {r#"
                    [logging]
                    log_directory = "/var/log/clockbound"

                    [logging.synchronization]
                    enabled = true

                    [logging.ffevents]
                    enabled = true
                "#},
            ),
            (
                "addresses, names and a pool",
                indoc! {r#"
                    [[sources.ntp]]
                    server = "192.0.2.1"

                    [[sources.ntp]]
                    server = "2001:db8::123"

                    [[sources.ntp]]
                    server = "ntp.corp.example"

                    [[sources.ntp]]
                    pool = "pool.ntp.example"
                "#},
            ),
            (
                "logging and sources together",
                indoc! {r#"
                    [logging]
                    log_directory = "/var/log/clockbound"

                    [logging.synchronization]
                    enabled = true

                    [[sources.ntp]]
                    server = "192.0.2.1"

                    [[sources.ntp]]
                    pool = "pool.ntp.example"
                "#},
            ),
        ] {
            parse(contents).unwrap_or_else(|e| panic!("expected {name:?} to be accepted: {e}"));
        }
    }

    /// Whole configurations that must be rejected, each for its own reason.
    #[test]
    fn configurations_rejected() {
        for (name, contents, expected) in [
            (
                "malformed syntax",
                indoc! {r#"
                    [logging]
                    log_directory = = "/var/log/clockbound"
                "#},
                "Invalid configuration syntax",
            ),
            (
                "unknown setting",
                indoc! {r#"
                    [logging]
                    log_directory = "/var/log/clockbound"
                    rotate = true
                "#},
                "unknown field `rotate`",
            ),
            (
                "both server and pool in one block",
                indoc! {r#"
                    [[sources.ntp]]
                    server = "ntp.corp.example"
                    pool = "pool.ntp.example"
                "#},
                "Invalid configuration syntax",
            ),
            (
                "enabled stream without a log directory",
                indoc! {r#"
                    [logging.ffevents]
                    enabled = true
                "#},
                "`log_directory` is required when any log stream is enabled",
            ),
            (
                "relative log directory",
                indoc! {r#"
                    [logging]
                    log_directory = "clockbound/logs"

                    [logging.ffevents]
                    enabled = true
                "#},
                "`log_directory` must be an absolute path",
            ),
            (
                "wrong type for a setting",
                indoc! {r#"
                    [logging.ffevents]
                    enabled = "yes"
                "#},
                "Invalid configuration syntax",
            ),
            (
                "a source block naming neither a server nor a pool",
                indoc! {r#"
                    [[sources.ntp]]
                "#},
                "Invalid configuration syntax",
            ),
            (
                "pool given an address",
                indoc! {r#"
                    [[sources.ntp]]
                    pool = "192.0.2.1"
                "#},
                "\"192.0.2.1\" is an IP address, not a name",
            ),
            (
                "pool given something that cannot be a name",
                indoc! {r#"
                    [[sources.ntp]]
                    pool = "192.0.2.999"
                "#},
                "\"192.0.2.999\" is not a valid name",
            ),
            (
                "source that is neither an address nor a name",
                indoc! {r#"
                    [[sources.ntp]]
                    server = "192.0.2.999"
                "#},
                "\"192.0.2.999\" is not an IP address or a valid name",
            ),
            (
                "more pools than are supported",
                indoc! {r#"
                    [[sources.ntp]]
                    pool = "a.example"

                    [[sources.ntp]]
                    pool = "b.example"
                "#},
                "at most 1 `pool` entry is supported, found 2",
            ),
            (
                "more servers than are supported",
                indoc! {r#"
                    [[sources.ntp]]
                    server = "192.0.2.1"

                    [[sources.ntp]]
                    server = "192.0.2.2"

                    [[sources.ntp]]
                    server = "192.0.2.3"

                    [[sources.ntp]]
                    server = "192.0.2.4"

                    [[sources.ntp]]
                    server = "192.0.2.5"

                    [[sources.ntp]]
                    server = "192.0.2.6"
                "#},
                "at most 5 `server` entries are supported, found 6",
            ),
            (
                "the same source twice",
                indoc! {r#"
                    [[sources.ntp]]
                    server = "ntp.corp.example"

                    [[sources.ntp]]
                    server = "ntp.corp.example"
                "#},
                "duplicate source value",
            ),
        ] {
            let error = parse(contents)
                .expect_err(&format!("expected {name:?} to be rejected"))
                .to_string();
            assert!(
                error.contains(expected),
                "{name:?}: expected {expected:?} in error, got: {error}"
            );
        }
    }

    #[test]
    fn empty_is_default() {
        assert_eq!(parse("").unwrap(), Config::default());
    }

    #[test]
    fn no_path_is_default() {
        assert_eq!(Config::load(None).unwrap(), Config::default());
    }

    #[test]
    fn example_config_parses_and_validates() {
        parse(EXAMPLE_CONFIG).unwrap();
    }

    #[test]
    fn serializing_renders_a_configuration_file() {
        // What serializing a configuration produces: the documented schema, with
        // defaults spelled out rather than omitted, and text that parses back. An
        // address is rendered rather than echoed, so `2001:0db8::1` comes back in
        // its canonical spelling.
        let config = parse(indoc! {r#"
            [logging]
            log_directory = "/var/log/clockbound"

            [logging.ffevents]
            enabled = true

            [[sources.ntp]]
            server = "2001:0db8::1"

            [[sources.ntp]]
            pool = "pool.ntp.example"
        "#})
        .unwrap();

        let rendered = toml::to_string(&config).unwrap();
        assert_eq!(
            rendered,
            indoc! {r#"
                [logging]
                log_directory = "/var/log/clockbound"

                [logging.synchronization]
                enabled = false

                [logging.ffevents]
                enabled = true

                [[sources.ntp]]
                server = "2001:db8::1"

                [[sources.ntp]]
                pool = "pool.ntp.example"
            "#},
        );
        assert_eq!(parse(&rendered).unwrap(), config);
    }

    #[test]
    fn round_trip() {
        let config = parse(EXAMPLE_CONFIG).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let reparsed: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(config, reparsed);
    }

    #[test]
    fn unknown_field_rejected_top_level() {
        assert!(matches!(parse("bogus = true"), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn unknown_field_rejected_nested() {
        assert!(matches!(
            parse(indoc! {r"
                [logging]
                bogus = true
            "}),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn unknown_field_rejected_in_stream() {
        assert!(matches!(
            parse(indoc! {r"
                [logging.ffevents]
                enabled = true
                bogus = 1
            "}),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn unknown_field_rejected_in_sources() {
        assert!(matches!(
            parse(indoc! {r"
                [sources]
                bogus = 1
            "}),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn unknown_key_in_ntp_source_rejected() {
        // An unknown key in an `[[sources.ntp]]` block is rejected as an unknown
        // variant (only `server`/`pool` are valid keys).
        assert!(matches!(
            parse(indoc! {r#"
                [[sources.ntp]]
                bogus = "x"
            "#}),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn parse_error_preserves_line_and_column() {
        // The `Parse` variant renders via `Display` so toml's line/column
        // snippet survives (a `{:?}` refactor would silently drop it).
        let ConfigError::Parse(_) = parse("bogus = = 1\n").unwrap_err() else {
            panic!("expected a Parse error");
        };
        let rendered = parse("bogus = = 1\n").unwrap_err().to_string();
        assert!(
            rendered.contains("line") && rendered.contains("column"),
            "expected a line/column snippet, got: {rendered}"
        );
    }

    #[test]
    fn type_error_rejected() {
        assert!(matches!(
            parse(indoc! {r#"
                [logging.ffevents]
                enabled = "yes"
            "#}),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn both_server_and_pool_rejected() {
        assert!(matches!(
            parse(indoc! {r#"
                [[sources.ntp]]
                server = "192.0.2.1"
                pool = "pool.ntp.example"
            "#}),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn neither_server_nor_pool_rejected() {
        assert!(matches!(
            parse("[[sources.ntp]]\n"),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn server_alone_accepted() {
        let (_, sources) = parse(indoc! {r#"
            [[sources.ntp]]
            server = "192.0.2.1"
        "#})
        .unwrap()
        .into_parts();
        assert_eq!(sources.ntp(), [server("192.0.2.1")]);
    }

    #[test]
    fn pool_alone_accepted() {
        let (_, sources) = parse(indoc! {r#"
            [[sources.ntp]]
            pool = "pool.ntp.example"
        "#})
        .unwrap()
        .into_parts();
        assert_eq!(sources.ntp(), [pool("pool.ntp.example")]);
    }

    // A value becomes a `Host` or a `DomainName` only if it meets their criteria,
    // so what each type accepts and rejects is the contract worth pinning.

    #[test]
    fn host_accepts_addresses_and_names() {
        for value in [
            "192.0.2.1",
            "2001:db8::123",
            // Addresses with their own notations: loopback, unspecified, and an
            // IPv4-mapped address.
            "::1",
            "::",
            "::ffff:192.0.2.1",
            "ntp.corp.example",
            "NTP.CORP.EXAMPLE",
            "time-1.example",
            "my_internal_ntp",
            "café.example",
            // A label may be entirely numeric as long as the final one is not
            // (RFC 1123 section 2.1), so these are names, not mistyped addresses.
            "1.example",
            "192.0.2.example",
            // A fully qualified name may end in the root dot.
            "ntp.corp.example.",
            // A single label is a name a resolver can still look up.
            "localhost",
        ] {
            Host::parse(value.to_string())
                .unwrap_or_else(|e| panic!("expected {value:?} to be accepted: {e}"));
        }
    }

    #[test]
    fn host_rejects_what_is_neither_address_nor_name() {
        for value in [
            // Mistyped addresses. Rust's parser is strict, so a short form or a
            // leading zero is not an address to it, and an all-numeric final label
            // keeps it from becoming a name either. `getaddrinfo` is lenient and
            // would answer with another address entirely — 10.0.0.0 for "10.0.0",
            // 8.0.0.1 for "010.0.0.1" read as octal — so they stop here.
            "192.0.2.999",
            "10.0.0",
            "010.0.0.1",
            "192.0.2999",
            "1.2.3.4.5",
            "192.0.2.1.",
            "12345",
            // A colon only ever appears in an IPv6 literal, never in a name.
            "2001:db8::gggg",
            "ntp.corp.example:123",
            // Whitespace between labels cannot be read as a name.
            "ntp .corp.example",
            "ntp\u{a0}corp.example",
            // Nothing usable at all.
            "",
            "   ",
            ".",
            "..",
            "bad\tname",
            "bad\nname",
        ] {
            let error = Host::parse(value.to_string())
                .expect_err(&format!("expected {value:?} to be rejected"));
            assert_eq!(
                error.to_string(),
                format!("{value:?} is not an IP address or a valid name")
            );
        }
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        for value in [
            " ntp.corp.example",
            "ntp.corp.example ",
            "\tntp.corp.example\n",
        ] {
            let name = DomainName::parse(value.to_string())
                .unwrap_or_else(|e| panic!("expected {value:?} to be accepted: {e}"));
            assert_eq!(name.as_str(), "ntp.corp.example");
        }
        // An address reads the same once trimmed.
        assert_eq!(
            Host::parse(" 192.0.2.1 ".to_string()).unwrap(),
            Host::Ip("192.0.2.1".parse().unwrap())
        );
    }

    #[test]
    fn domain_name_rejects_an_address() {
        for value in ["192.0.2.1", "2001:db8::123"] {
            let error = DomainName::parse(value.to_string())
                .expect_err(&format!("expected {value:?} to be rejected"));
            assert_eq!(
                error.to_string(),
                format!("{value:?} is an IP address, not a name")
            );
        }
    }

    #[test]
    fn domain_name_rejects_what_cannot_be_a_name() {
        for value in ["", "   ", "bad\tname", "192.0.2.999", "12345", "1:2"] {
            let error = DomainName::parse(value.to_string())
                .expect_err(&format!("expected {value:?} to be rejected"));
            assert_eq!(error.to_string(), format!("{value:?} is not a valid name"));
        }
    }

    #[test]
    fn exactly_max_servers_accepted() {
        // The cap is `> MAX_SERVERS`, so exactly MAX_SERVERS must pass.
        let ntp = (0..MAX_SERVERS)
            .map(|i| server(&format!("10.0.0.{i}")))
            .collect();
        let config = Config {
            sources: SourcesConfig { ntp },
            ..Config::default()
        };
        config.validate().unwrap();
    }

    // Semantic rejections: assert the full customer-facing message verbatim.

    #[test]
    fn too_many_servers_rejected() {
        let ntp = (0..=MAX_SERVERS)
            .map(|i| server(&format!("10.0.0.{i}")))
            .collect();
        let config = Config {
            sources: SourcesConfig { ntp },
            ..Config::default()
        };
        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "Invalid configuration:\n  - at most 5 `server` entries are supported, found 6",
        );
    }

    #[test]
    fn too_many_pools_rejected() {
        let config = Config {
            sources: SourcesConfig {
                ntp: vec![pool("a.example"), pool("b.example")],
            },
            ..Config::default()
        };
        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "Invalid configuration:\n  - at most 1 `pool` entry is supported, found 2",
        );
    }

    #[test]
    fn duplicate_source_rejected() {
        let config = Config {
            sources: SourcesConfig {
                ntp: vec![server("192.0.2.1"), server("192.0.2.1")],
            },
            ..Config::default()
        };
        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "Invalid configuration:\n  - duplicate source value: \"192.0.2.1\"",
        );
    }

    #[test]
    fn same_name_as_a_server_and_a_pool_is_a_duplicate() {
        // Sources are compared by value, not by the key that carried them, so
        // naming one twice is caught even across `server` and `pool`.
        let config = Config {
            sources: SourcesConfig {
                ntp: vec![server("ntp.corp.example"), pool("ntp.corp.example")],
            },
            ..Config::default()
        };
        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "Invalid configuration:\n  - duplicate source value: \"ntp.corp.example\"",
        );
    }

    #[test]
    fn enabled_stream_requires_log_directory() {
        let config = Config {
            logging: LoggingConfig {
                log_directory: None,
                ffevents: StreamConfig { enabled: true },
                ..LoggingConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "Invalid configuration:\n  - `log_directory` is required when any log stream is enabled",
        );
    }

    #[test]
    fn relative_log_directory_is_rejected() {
        // No stream is enabled: a relative directory is rejected wherever it
        // appears, not only where one would be written to.
        let config = Config {
            logging: LoggingConfig {
                log_directory: Some(PathBuf::from("relative/path")),
                ..LoggingConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "Invalid configuration:\n  - `log_directory` must be an absolute path: relative/path",
        );
    }

    #[test]
    fn disabled_streams_need_no_log_directory() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn reports_every_violation() {
        // Trips cardinality, a duplicate source, and a missing log_directory at
        // once. Each message is pinned by its own test; what matters here is that
        // none of them is dropped.
        let config = Config {
            logging: LoggingConfig {
                ffevents: StreamConfig { enabled: true },
                ..LoggingConfig::default()
            },
            sources: SourcesConfig {
                ntp: vec![
                    server("ntp.corp.example"),
                    server("ntp.corp.example"),
                    pool("a.example"),
                    pool("b.example"),
                ],
            },
        };
        let error = config.validate().unwrap_err().to_string();
        for expected in [
            "at most 1 `pool` entry is supported, found 2",
            "duplicate source value: \"ntp.corp.example\"",
            "`log_directory` is required when any log stream is enabled",
        ] {
            assert!(
                error.contains(expected),
                "expected {expected:?} in: {error}"
            );
        }
    }

    #[test]
    fn missing_explicit_file_is_not_found() {
        assert_eq!(
            Config::load(Some(PathBuf::from("/nonexistent/clockbound.toml")))
                .unwrap_err()
                .to_string(),
            "Configuration file not found: /nonexistent/clockbound.toml",
        );
    }

    #[test]
    fn load_reads_parses_and_validates_a_file() {
        use std::io::Write as _;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            indoc! {r#"
                [logging]
                log_directory = "/var/log/clockbound"

                [logging.ffevents]
                enabled = true
            "#}
        )
        .unwrap();

        let (logging, _) = Config::load(Some(file.path().to_path_buf()))
            .unwrap()
            .into_parts();
        assert!(logging.ffevents().is_enabled());
        assert_eq!(
            logging.log_directory(),
            Some(Path::new("/var/log/clockbound"))
        );
    }

    #[test]
    fn load_rejects_a_semantic_violation() {
        // Parseable and schema-clean, so only the semantic pass rejects it.
        use std::io::Write as _;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            indoc! {r#"
                [[sources.ntp]]
                pool = "a.example"

                [[sources.ntp]]
                pool = "b.example"
            "#}
        )
        .unwrap();

        assert_eq!(
            Config::load(Some(file.path().to_path_buf()))
                .unwrap_err()
                .to_string(),
            "Invalid configuration:\n  - at most 1 `pool` entry is supported, found 2",
        );
    }

    #[test]
    fn unreadable_file_reported() {
        // A directory exists but cannot be read as a file so its error kind
        // is not `NotFound`, so it maps to `Unreadable`.
        let dir = tempfile::tempdir().unwrap();
        let err = Config::load(Some(dir.path().to_path_buf())).unwrap_err();
        assert!(matches!(err, ConfigError::Unreadable { .. }));
    }
}
