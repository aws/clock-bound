//! Struct holding instance type meta data, sourced from the local DMI/SMBIOS tables.
//!
//! Determines, during initialization, (1) whether this host is an EC2 instance and (2) whether it
//! is bare metal. Both are used to decide whether the VMClock device should be enabled (it is
//! disabled on non-EC2 hosts and on bare-metal instances).
//!
//! Unlike the Instance Metadata Service (a network call to `169.254.169.254`), the DMI tables are
//! exposed by the kernel as plain files under `/sys/devices/virtual/dmi/id/`, so this is a fast,
//! local, dependency-free read that cannot time out or be blocked by the network:
//!
//! - `sys_vendor` — reads `Amazon EC2` on instances built on the Nitro System (our primary EC2
//!   signal), and `Xen` on older pre-Nitro instances.
//! - `product_name` — on Nitro this is the instance type string, e.g. `m5zn.12xlarge` or
//!   `m7i.metal-24xlarge` (the same value IMDS returns for `instance-type`; our metal signal).
//!
//! ## The Xen path
//!
//! Pre-Nitro instances (e.g. `t2`, `m4`, `c4`, `r4`) run on the Xen hypervisor and report
//! `sys_vendor = "Xen"`, **not** `Amazon EC2`. Their `product_name` is the generic `HVM domU`,
//! which carries no instance-type information. So the vendor string alone would misclassify a
//! legitimate EC2 instance as non-EC2. To recognise these, we additionally check
//! `/sys/hypervisor/uuid`: on an EC2 Xen guest it is prefixed with `ec2` (verified empirically
//! across regions, and documented as the UUID method below). This prefix is a heuristic, not a
//! cryptographic proof — for a trust boundary use the signed instance identity document instead.
//!
//! Xen instances are pre-Nitro and are never bare metal (metal is a Nitro-only capability), so
//! `is_metal()` on their `HVM domU` product name correctly stays `false`.
//!
//! See [Detect whether a host is an EC2 instance] in the EC2 User Guide.
//!
//! [Detect whether a host is an EC2 instance]:
//! https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/identify_ec2_instances.html

use std::fs;
use std::path::Path;

use thiserror::Error;

/// Failure reading the local DMI tables.
///
/// A proven non-EC2 host is deliberately **not** modeled as an error: it is a normal answer
/// (`Ok(None)` from [`InstanceType::get_from_dmi`]). This type therefore only represents an
/// inability to read a required DMI file.
#[derive(Debug, Error)]
#[error("unable to read DMI table")]
pub struct DmiError {
    #[from]
    pub source: std::io::Error,
}

/// The `sys_vendor` value reported by the kernel, classified into the cases we care about.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Vendor {
    /// `Amazon EC2` — a Nitro-based EC2 instance.
    AmazonEc2,
    /// `Xen` — a pre-Nitro instance (EC2 or otherwise; confirmed via the hypervisor UUID).
    Xen,
    /// Any other vendor string; not an EC2 instance.
    Other,
}

impl Vendor {
    /// The `sys_vendor` string reported by a Nitro-based EC2 instance.
    const AMAZON_EC2: &'static str = "Amazon EC2";
    /// The `sys_vendor` string reported by a pre-Nitro (Xen) instance.
    const XEN: &'static str = "Xen";

    fn from_sys_vendor(sys_vendor: &str) -> Self {
        match sys_vendor.trim() {
            Self::AMAZON_EC2 => Vendor::AmazonEc2,
            Self::XEN => Vendor::Xen,
            _ => Vendor::Other,
        }
    }
}

/// Instance type meta data used to determine instance capabilities.
///
/// The inner string is private so callers cannot construct or mutate it into a nonsensical value;
/// obtain one via [`Self::get_from_dmi`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceType(String);

impl InstanceType {
    /// Directory under which the kernel exposes the DMI/SMBIOS tables.
    const DMI_DIR: &'static str = "/sys/devices/virtual/dmi/id";
    /// System vendor file; reads `Amazon EC2` on Nitro instances, `Xen` on pre-Nitro instances.
    const VENDOR_FILE: &'static str = "sys_vendor";
    /// Product name file; holds the instance type string (e.g. `m5zn.12xlarge`) on Nitro.
    const PRODUCT_NAME_FILE: &'static str = "product_name";
    /// Hypervisor UUID exposed on Xen guests. Note: this lives outside [`Self::DMI_DIR`], and does
    /// not exist on Nitro instances (which have no `/sys/hypervisor`).
    const HYPERVISOR_UUID_PATH: &'static str = "/sys/hypervisor/uuid";
    /// `product_name` reported by Xen guests (generic; carries no instance-type info).
    const XEN_PRODUCT_NAME: &'static str = "HVM domU";
    /// Prefix of `/sys/hypervisor/uuid` on an EC2 Xen guest.
    const EC2_UUID_PREFIX: &'static str = "ec2";

    /// Read instance type meta data from the local DMI tables.
    ///
    /// Branches on `sys_vendor`:
    /// 1. `Amazon EC2` (Nitro) — trust `product_name` as the instance type string, from which
    ///    [`Self::is_metal`] is derived. Yields `Ok(Some(_))`.
    /// 2. `Xen` (pre-Nitro) — `product_name` is the generic `HVM domU`, which carries no
    ///    instance-type information, so confirm this is really EC2 via the `ec2`-prefixed
    ///    `/sys/hypervisor/uuid`. If confirmed, return the Xen product name as-is (never metal),
    ///    `Ok(Some(_))`. If the UUID does not match, treat as non-EC2.
    /// 3. Anything else — non-EC2, returned as `Ok(None)`.
    ///
    /// # Errors
    /// Returns [`DmiError`] if a required DMI file cannot be read.
    pub fn get_from_dmi() -> Result<Option<Self>, DmiError> {
        Self::get_from_dmi_at(
            Path::new(Self::DMI_DIR),
            Path::new(Self::HYPERVISOR_UUID_PATH),
        )
    }

    /// [`Self::get_from_dmi`] with the DMI base directory and hypervisor-UUID path injected, so
    /// tests can point it at a fabricated `/sys` layout. Production always calls it via
    /// [`Self::get_from_dmi`] with the real kernel paths.
    fn get_from_dmi_at(
        dmi_dir: &Path,
        hypervisor_uuid_path: &Path,
    ) -> Result<Option<Self>, DmiError> {
        let sys_vendor = Self::read_dmi_field_at(dmi_dir, Self::VENDOR_FILE)?;

        match Vendor::from_sys_vendor(&sys_vendor) {
            Vendor::AmazonEc2 => {
                // Nitro: product_name is the instance type string; metal is derived from it.
                let product_name = Self::read_dmi_field_at(dmi_dir, Self::PRODUCT_NAME_FILE)?;
                Ok(Some(Self(product_name)))
            }
            Vendor::Xen => {
                // Pre-Nitro Xen: the vendor string alone can't distinguish an EC2 Xen guest from
                // any other Xen VM, so confirm via the hypervisor UUID. A read failure here is
                // treated as "not confirmed EC2" rather than a hard IO error — the file may be
                // absent.
                let uuid = fs::read_to_string(hypervisor_uuid_path).unwrap_or_default();
                if Self::is_ec2_xen_uuid(&uuid) {
                    Ok(Some(Self(Self::XEN_PRODUCT_NAME.to_string())))
                } else {
                    Ok(None)
                }
            }
            Vendor::Other => Ok(None),
        }
    }

    /// Whether a `/sys/hypervisor/uuid` value identifies an EC2 Xen guest (prefix `ec2`,
    /// case-insensitive). Heuristic, not a cryptographic proof of EC2.
    fn is_ec2_xen_uuid(hypervisor_uuid: &str) -> bool {
        hypervisor_uuid
            .trim()
            .to_ascii_lowercase()
            .starts_with(Self::EC2_UUID_PREFIX)
    }

    /// Whether this instance is bare metal. VMClock is not available on metal instances.
    pub fn is_metal(&self) -> bool {
        self.0.contains("metal")
    }

    /// Read and trim a single DMI field from `dmi_dir`.
    fn read_dmi_field_at(dmi_dir: &Path, file: &str) -> std::io::Result<String> {
        let path = dmi_dir.join(file);
        Ok(fs::read_to_string(path)?.trim().to_string())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::fs::write;
    use tempfile::tempdir;

    /// Build a fabricated DMI directory containing the given field files, returning the tempdir
    /// (kept alive by the caller) and its path. Each `(file, contents)` pair is written verbatim.
    fn dmi_dir_with(fields: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempdir().expect("create temp DMI dir");
        for (file, contents) in fields {
            write(dir.path().join(file), contents).expect("write DMI field");
        }
        dir
    }

    #[test]
    fn ec2_vendor_is_recognized() {
        assert_eq!(Vendor::from_sys_vendor("Amazon EC2"), Vendor::AmazonEc2);
    }

    #[test]
    fn ec2_vendor_tolerates_trailing_whitespace() {
        // DMI files are commonly newline-terminated.
        assert_eq!(Vendor::from_sys_vendor("Amazon EC2\n"), Vendor::AmazonEc2);
    }

    #[test]
    fn vendor_classification() {
        assert_eq!(Vendor::from_sys_vendor("Xen"), Vendor::Xen);
        assert_eq!(Vendor::from_sys_vendor("QEMU"), Vendor::Other);
        assert_eq!(Vendor::from_sys_vendor(""), Vendor::Other);
    }

    #[test]
    fn metal_instance() {
        let instance_type = InstanceType("m7i.metal-24xlarge".to_string());

        assert!(instance_type.is_metal())
    }

    #[test]
    fn nonmetal_instance() {
        let instance_type = InstanceType("m5zn.12xlarge".to_string());

        assert!(!instance_type.is_metal())
    }

    #[test]
    fn ec2_xen_uuid_is_recognized() {
        // Real EC2 Xen guests expose an `ec2`-prefixed hypervisor UUID.
        assert!(InstanceType::is_ec2_xen_uuid("ec26d825-3b17-...\n"));
        // Case-insensitive: some sources report uppercase.
        assert!(InstanceType::is_ec2_xen_uuid("EC26D825-3B17"));
    }

    #[test]
    fn non_ec2_xen_uuid_is_rejected() {
        // A non-EC2 Xen VM (or empty read) must not be treated as EC2.
        assert!(!InstanceType::is_ec2_xen_uuid("abcd1234-generic-xen"));
        assert!(!InstanceType::is_ec2_xen_uuid(""));
    }

    #[test]
    fn xen_product_name_is_not_metal() {
        // The Xen path yields the generic `HVM domU`; it must never look metal.
        let instance_type = InstanceType(InstanceType::XEN_PRODUCT_NAME.to_string());

        assert!(!instance_type.is_metal())
    }

    // --- get_from_dmi_at branch coverage (deterministic on any host) ---

    #[test]
    fn dmi_nitro_nonmetal_returns_instance_type() {
        let dmi = dmi_dir_with(&[
            ("sys_vendor", "Amazon EC2\n"),
            ("product_name", "m5zn.12xlarge\n"),
        ]);
        // No hypervisor UUID on Nitro; point at a nonexistent path.
        let result = InstanceType::get_from_dmi_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert_eq!(
            result.unwrap(),
            Some(InstanceType("m5zn.12xlarge".to_string()))
        );
    }

    #[test]
    fn dmi_nitro_metal_returns_metal_instance_type() {
        let dmi = dmi_dir_with(&[
            ("sys_vendor", "Amazon EC2\n"),
            ("product_name", "m7i.metal-24xlarge\n"),
        ]);
        let result = InstanceType::get_from_dmi_at(dmi.path(), Path::new("/nonexistent/uuid"));
        let instance_type = result.unwrap().expect("EC2 instance type");
        assert!(instance_type.is_metal());
    }

    #[test]
    fn dmi_xen_with_ec2_uuid_returns_hvm_domu() {
        // Xen vendor, generic product name, but an ec2-prefixed hypervisor UUID confirms EC2.
        let dmi = dmi_dir_with(&[("sys_vendor", "Xen\n"), ("product_name", "HVM domU\n")]);
        let uuid_dir = dmi_dir_with(&[("uuid", "ec26d825-3b17-deadbeef\n")]);
        let uuid_path = uuid_dir.path().join("uuid");

        let result = InstanceType::get_from_dmi_at(dmi.path(), &uuid_path);
        assert_eq!(
            result.unwrap(),
            Some(InstanceType(InstanceType::XEN_PRODUCT_NAME.to_string()))
        );
    }

    #[test]
    fn dmi_xen_without_ec2_uuid_is_non_ec2() {
        // Xen vendor but a non-ec2 UUID: a generic Xen VM, not EC2 → Ok(None).
        let dmi = dmi_dir_with(&[("sys_vendor", "Xen\n"), ("product_name", "HVM domU\n")]);
        let uuid_dir = dmi_dir_with(&[("uuid", "abcd1234-generic-xen\n")]);
        let uuid_path = uuid_dir.path().join("uuid");

        let result = InstanceType::get_from_dmi_at(dmi.path(), &uuid_path);
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn dmi_xen_missing_uuid_is_non_ec2() {
        // Xen vendor but the UUID file is absent: the read defaults to empty → not confirmed EC2.
        let dmi = dmi_dir_with(&[("sys_vendor", "Xen\n"), ("product_name", "HVM domU\n")]);
        let result = InstanceType::get_from_dmi_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn dmi_other_vendor_is_non_ec2() {
        let dmi = dmi_dir_with(&[("sys_vendor", "QEMU\n")]);
        let result = InstanceType::get_from_dmi_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn dmi_unreadable_vendor_is_error() {
        // sys_vendor file absent → the read fails → DmiError, NOT a non-EC2 verdict.
        let dmi = dmi_dir_with(&[]);
        let result = InstanceType::get_from_dmi_at(dmi.path(), Path::new("/nonexistent/uuid"));
        assert!(matches!(result, Err(DmiError { .. })));
    }
}
