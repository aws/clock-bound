//! Platform auto-detection for the ClockBound daemon.
//!
//! Determines at startup whether this host is an Amazon EC2 instance and, if so, what hypervisor
//! generation it runs on. The result drives which clock sources the daemon installs:
//!
//! - **Amazon (Nitro or Xen):** Amazon Time Sync, PHC, and (if non-metal) VMClock.
//! - **Other:** Only NTP sources.

pub mod amazon;
pub use amazon::Amazon;

use std::io::ErrorKind;
use std::path::Path;

use thiserror::Error;

/// Failure during platform auto-detection.
#[derive(Debug, Error)]
#[error("platform auto-detection failed")]
pub struct AutodetectError {
    #[from]
    source: std::io::Error,
}

/// The detected platform this host is running on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Autodetect {
    /// An Amazon EC2 instance (Nitro or pre-Nitro Xen).
    Amazon(Amazon),
    /// Not an EC2 instance, or platform could not be confirmed.
    Other,
}

impl Autodetect {
    /// Directory under which the kernel exposes the DMI/SMBIOS tables.
    const DMI_DIR: &'static str = "/sys/devices/virtual/dmi/id";
    /// Hypervisor UUID exposed on Xen guests; does not exist on Nitro instances.
    const HYPERVISOR_UUID_PATH: &'static str = "/sys/hypervisor/uuid";

    /// Detect the platform from the local DMI/SMBIOS tables.
    ///
    /// # Errors
    /// Returns [`AutodetectError`] if a required DMI file exists but cannot be read (permission
    /// denied, IO fault). A missing file yields [`Autodetect::Other`].
    pub fn detect() -> Result<Self, AutodetectError> {
        Self::detect_at(
            Path::new(Self::DMI_DIR),
            Path::new(Self::HYPERVISOR_UUID_PATH),
        )
    }

    /// Testable entry point with injected filesystem paths.
    fn detect_at(dmi_dir: &Path, hypervisor_uuid_path: &Path) -> Result<Self, AutodetectError> {
        let sys_vendor = match read_dmi_field(dmi_dir, "sys_vendor") {
            Ok(value) => value,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // If this does not exist, it's not an EC2 linux instance
                return Ok(Autodetect::Other);
            }
            Err(e) => return Err(AutodetectError::from(e)),
        };

        match sys_vendor.trim() {
            Amazon::SYS_VENDOR_NITRO => match Amazon::detect_nitro_at(dmi_dir)? {
                Some(amazon) => Ok(Autodetect::Amazon(amazon)),
                None => Ok(Autodetect::Other),
            },
            Amazon::SYS_VENDOR_XEN => match Amazon::detect_xen_at(hypervisor_uuid_path) {
                Some(amazon) => Ok(Autodetect::Amazon(amazon)),
                None => Ok(Autodetect::Other),
            },
            _ => Ok(Autodetect::Other),
        }
    }
}

/// Read and trim a single DMI field from the given directory.
fn read_dmi_field(dmi_dir: &Path, file: &str) -> std::io::Result<String> {
    let path = dmi_dir.join(file);
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::write;
    use tempfile::tempdir;

    /// Build a fabricated DMI directory containing the given field files.
    fn dmi_dir_with(fields: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempdir().expect("create temp DMI dir");
        for (file, contents) in fields {
            write(dir.path().join(file), contents).expect("write DMI field");
        }
        dir
    }

    #[test]
    fn nitro_nonmetal_detected() {
        let dmi = dmi_dir_with(&[
            ("sys_vendor", "Amazon EC2\n"),
            ("product_name", "m5zn.12xlarge\n"),
        ]);
        let result = Autodetect::detect_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert_eq!(
            result.unwrap(),
            Autodetect::Amazon(Amazon::Nitro("m5zn.12xlarge".to_string()))
        );
    }

    #[test]
    fn nitro_metal_detected() {
        let dmi = dmi_dir_with(&[
            ("sys_vendor", "Amazon EC2\n"),
            ("product_name", "m7i.metal-24xlarge\n"),
        ]);
        let result = Autodetect::detect_at(dmi.path(), Path::new("/nonexistent/uuid"));
        let autodetect = result.unwrap();
        assert_eq!(
            autodetect,
            Autodetect::Amazon(Amazon::Nitro("m7i.metal-24xlarge".to_string()))
        );
        if let Autodetect::Amazon(ref amazon) = autodetect {
            assert!(amazon.is_metal());
        }
    }

    #[test]
    fn xen_with_ec2_uuid_detected() {
        let dmi = dmi_dir_with(&[("sys_vendor", "Xen\n")]);
        let uuid_dir = dmi_dir_with(&[("uuid", "ec26d825-3b17-deadbeef\n")]);
        let uuid_path = uuid_dir.path().join("uuid");

        let result = Autodetect::detect_at(dmi.path(), &uuid_path);
        assert_eq!(result.unwrap(), Autodetect::Amazon(Amazon::Xen));
    }

    #[test]
    fn xen_without_ec2_uuid_is_other() {
        let dmi = dmi_dir_with(&[("sys_vendor", "Xen\n")]);
        let uuid_dir = dmi_dir_with(&[("uuid", "abcd1234-generic-xen\n")]);
        let uuid_path = uuid_dir.path().join("uuid");

        let result = Autodetect::detect_at(dmi.path(), &uuid_path);
        assert_eq!(result.unwrap(), Autodetect::Other);
    }

    #[test]
    fn xen_missing_uuid_is_other() {
        let dmi = dmi_dir_with(&[("sys_vendor", "Xen\n")]);
        let result = Autodetect::detect_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert_eq!(result.unwrap(), Autodetect::Other);
    }

    #[test]
    fn other_vendor_is_other() {
        let dmi = dmi_dir_with(&[("sys_vendor", "QEMU\n")]);
        let result = Autodetect::detect_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert_eq!(result.unwrap(), Autodetect::Other);
    }

    #[test]
    fn missing_sys_vendor_is_other() {
        // Container or minimal environment without /sys/devices/virtual/dmi/id/sys_vendor.
        let dmi = dmi_dir_with(&[]);
        let result = Autodetect::detect_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert_eq!(result.unwrap(), Autodetect::Other);
    }

    #[test]
    fn vendor_tolerates_trailing_whitespace() {
        let dmi = dmi_dir_with(&[
            ("sys_vendor", "Amazon EC2\n"),
            ("product_name", "c5.xlarge\n"),
        ]);
        let result = Autodetect::detect_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert_eq!(
            result.unwrap(),
            Autodetect::Amazon(Amazon::Nitro("c5.xlarge".to_string()))
        );
    }
}
