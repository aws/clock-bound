//! Amazon EC2 platform detection and classification.
//!
//! Determines whether a host is an Amazon EC2 instance
//!
//! ## How `clockbound` reacts to being detected as Amazon
//!
//! If detected as an Amazon EC2 instance, `clockbound` will install the following sources
//! - the local Amazon Time Sync Service via NTP
//! - the ptp hardware clock (phc)
//! - the VMClock (if non-metal, aka a virtualized instance)
//!
//! For more information on the time tracking sources, see
//! <https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configure-ec2-ntp.html>.
//!
//! ## Detection logic flow
//!
//! Detects if this is an EC2 instance by using DMI calls.
//! For more reference, see public docs to detect a host as EC2 here:
//! <https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/identify_ec2_instances.html>.
//!
//! Currently, Amazon EC2 instance detection is only supported on Linux instances

use std::path::Path;

use super::AutodetectError;

/// Prefix of `/sys/hypervisor/uuid` on an EC2 Xen guest.
const EC2_UUID_PREFIX: &str = "ec2";

/// An Amazon EC2 instance
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Amazon {
    /// Nitro-based EC2 instance. The string is the instance type (e.g., `m5zn.12xlarge`).
    Nitro(String),
    /// Pre-Nitro Xen-based EC2 instance.
    Xen,
}

impl Amazon {
    /// The `sys_vendor` string reported by a Nitro-based EC2 instance.
    pub(super) const SYS_VENDOR_NITRO: &'static str = "Amazon EC2";

    /// The `sys_vendor` string reported by a Xen EC2 instance.
    pub(super) const SYS_VENDOR_XEN: &'static str = "Xen";

    /// Whether this instance is bare metal.
    ///
    /// Bare-metal instances do not have a VMClock device.
    pub fn is_metal(&self) -> bool {
        match self {
            Amazon::Nitro(instance_type) => instance_type.contains("metal"),
            Amazon::Xen => false,
        }
    }

    /// Detect a Nitro instance from the DMI `product_name` field.
    ///
    /// Called when `sys_vendor` has already been confirmed as `"Amazon EC2"`.
    ///
    /// # Returns
    /// - `Ok(Some(Amazon::Nitro(...)))` on success.
    /// - `Ok(None)` if `product_name` is missing (shouldn't happen on Nitro, but defensive).
    ///
    /// # Errors
    /// Returns [`AutodetectError`] if `product_name` exists but cannot be read.
    pub(super) fn detect_nitro_at(dmi_dir: &Path) -> Result<Option<Self>, AutodetectError> {
        let product_name = match super::read_dmi_field(dmi_dir, "product_name") {
            Ok(value) => value,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(AutodetectError::from(e)),
        };
        Ok(Some(Amazon::Nitro(product_name)))
    }

    /// Detect a Xen-based EC2 instance by checking the hypervisor UUID.
    ///
    /// Called when `sys_vendor` is `"Xen"`. On an EC2 Xen guest, `/sys/hypervisor/uuid` is
    /// prefixed with `ec2`. This is a heuristic, not a cryptographic proof.
    ///
    /// # Returns
    /// - `Some(Amazon::Xen)` if the UUID confirms EC2.
    /// - `None` if the UUID doesn't match or the file is missing/unreadable.
    pub(super) fn detect_xen_at(hypervisor_uuid_path: &Path) -> Option<Self> {
        let uuid = std::fs::read_to_string(hypervisor_uuid_path).unwrap_or_default();
        if is_ec2_xen_uuid(&uuid) {
            Some(Amazon::Xen)
        } else {
            None
        }
    }
}

/// Whether a `/sys/hypervisor/uuid` value identifies an EC2 Xen guest (prefix `ec2`,
/// case-insensitive).
fn is_ec2_xen_uuid(hypervisor_uuid: &str) -> bool {
    hypervisor_uuid
        .trim()
        .to_ascii_lowercase()
        .starts_with(EC2_UUID_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::write;
    use tempfile::tempdir;

    fn dmi_dir_with(fields: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempdir().expect("create temp dir");
        for (file, contents) in fields {
            write(dir.path().join(file), contents).expect("write field");
        }
        dir
    }

    #[test]
    fn nitro_metal_is_metal() {
        let amazon = Amazon::Nitro("m7i.metal-24xlarge".to_string());
        assert!(amazon.is_metal());
    }

    #[test]
    fn nitro_nonmetal_is_not_metal() {
        let amazon = Amazon::Nitro("m5zn.12xlarge".to_string());
        assert!(!amazon.is_metal());
    }

    #[test]
    fn xen_is_not_metal() {
        assert!(!Amazon::Xen.is_metal());
    }

    #[test]
    fn detect_nitro_reads_product_name() {
        let dmi = dmi_dir_with(&[("product_name", "c5.xlarge\n")]);
        let result = Amazon::detect_nitro_at(dmi.path());
        assert_eq!(
            result.unwrap(),
            Some(Amazon::Nitro("c5.xlarge".to_string()))
        );
    }

    #[test]
    fn detect_nitro_missing_product_name_is_none() {
        let dmi = dmi_dir_with(&[]);
        let result = Amazon::detect_nitro_at(dmi.path());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn detect_xen_with_ec2_uuid() {
        let uuid_dir = dmi_dir_with(&[("uuid", "ec26d825-3b17-deadbeef\n")]);
        let uuid_path = uuid_dir.path().join("uuid");
        assert_eq!(Amazon::detect_xen_at(&uuid_path), Some(Amazon::Xen));
    }

    #[test]
    fn detect_xen_with_non_ec2_uuid() {
        let uuid_dir = dmi_dir_with(&[("uuid", "abcd1234-generic\n")]);
        let uuid_path = uuid_dir.path().join("uuid");
        assert_eq!(Amazon::detect_xen_at(&uuid_path), None);
    }

    #[test]
    fn detect_xen_missing_uuid_file() {
        assert_eq!(Amazon::detect_xen_at(Path::new("/nonexistent/uuid")), None);
    }

    #[test]
    fn ec2_xen_uuid_case_insensitive() {
        assert!(is_ec2_xen_uuid("EC26D825-3B17"));
        assert!(is_ec2_xen_uuid("ec26d825-3b17-...\n"));
    }

    #[test]
    fn non_ec2_uuid_rejected() {
        assert!(!is_ec2_xen_uuid("abcd1234"));
        assert!(!is_ec2_xen_uuid(""));
    }
}
