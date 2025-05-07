#[cfg_attr(any(test, feature = "test"), mockall::automock)]
mod get_pci_slot {
    /// Gets the PCI slot name for a given network interface name.
    ///
    /// # Arguments
    ///
    /// * `uevent_file_path` - The path of the uevent file where we lookup the PCI_SLOT_NAME.
    pub(crate) fn get_pci_slot_name(uevent_file_path: &str) -> anyhow::Result<String> {
        let contents = std::fs::read_to_string(uevent_file_path).map_err(|e| {
            anyhow::anyhow!(
                "Failed to open uevent file {:?} for PHC network interface specified: {}",
                uevent_file_path,
                e
            )
        })?;

        Ok(contents
            .lines()
            .find_map(|line| line.strip_prefix("PCI_SLOT_NAME="))
            .ok_or(anyhow::anyhow!(
                "Failed to find PCI_SLOT_NAME at uevent file path {:?}",
                uevent_file_path
            ))?
            .to_string())
    }
}

#[cfg(not(any(test, feature = "test")))]
pub(crate) use get_pci_slot::get_pci_slot_name;
#[cfg(any(test, feature = "test"))]
pub(crate) use mock_get_pci_slot::get_pci_slot_name;

/// Gets the PHC Error Bound sysfs file path given a network interface name.
///
/// # Arguments
///
/// * `interface` - The network interface to lookup the PHC error bound path for.
pub fn get_error_bound_sysfs_path(interface: &str) -> anyhow::Result<std::path::PathBuf> {
    let uevent_file_path = format!("/sys/class/net/{}/device/uevent", interface);
    let pci_slot_name = get_pci_slot_name(&uevent_file_path)?;
    Ok(std::path::PathBuf::from(format!(
        "/sys/bus/pci/devices/{}/phc_error_bound",
        pci_slot_name
    )))
}

pub struct PhcWithSysfsErrorBound {
    sysfs_phc_error_bound_path: std::path::PathBuf,
    phc_ref_id: u32,
}

#[cfg_attr(any(test, feature = "test"), mockall::automock)]
impl PhcWithSysfsErrorBound {
    pub(crate) fn new(phc_error_bound_path: std::path::PathBuf, phc_ref_id: u32) -> Self {
        Self {
            sysfs_phc_error_bound_path: phc_error_bound_path,
            phc_ref_id,
        }
    }

    pub(crate) fn read_phc_error_bound(&self) -> anyhow::Result<i64> {
        std::fs::read_to_string(&self.sysfs_phc_error_bound_path)?
            .trim()
            .parse::<i64>()
            .map_err(|e| anyhow::anyhow!("Failed to parse PHC error bound value to i64: {}", e))
    }

    pub(crate) fn get_phc_ref_id(&self) -> u32 {
        self.phc_ref_id
    }
}

#[cfg(test)]
mod test {
    use rstest::rstest;
    use tempfile::NamedTempFile;

    use super::*;

    use std::io::Write;

    #[rstest]
    #[case::happy_path("PCI_SLOT_NAME=12345", "12345")]
    #[case::happy_path_multi_line(
        "
oneline
PCI_SLOT_NAME=23456
twoline",
        "23456"
    )]
    fn test_get_pci_slot_name_success(
        #[case] file_contents_to_write: &str,
        #[case] return_value: &str,
    ) {
        let mut test_uevent_file = NamedTempFile::new().expect("create mock uevent file failed");
        test_uevent_file
            .write_all(file_contents_to_write.as_bytes())
            .expect("write to mock uevent file failed");

        let rt = get_pci_slot::get_pci_slot_name(test_uevent_file.path().to_str().unwrap());
        assert!(rt.is_ok());
        assert_eq!(rt.unwrap(), return_value.to_string());
    }

    #[rstest]
    #[case::missing_pci_slot_name("no pci slot name")]
    fn test_get_pci_slot_name_failure(#[case] file_contents_to_write: &str) {
        let mut test_uevent_file = NamedTempFile::new().expect("create mock uevent file failed");
        test_uevent_file
            .write_all(file_contents_to_write.as_bytes())
            .expect("write to mock uevent file failed");

        let rt = get_pci_slot::get_pci_slot_name(test_uevent_file.path().to_str().unwrap());
        assert!(rt.is_err());
        assert!(rt
            .unwrap_err()
            .to_string()
            .contains("Failed to find PCI_SLOT_NAME at uevent file path"));
    }

    #[test]
    fn test_get_pci_slot_name_file_does_not_exist() {
        let rt = get_pci_slot::get_pci_slot_name("/does/not/exist");
        assert!(rt.is_err());
    }

    #[rstest]
    #[case::happy_path("12345", 12345)]
    fn test_read_phc_error_bound_success(
        #[case] file_contents_to_write: &str,
        #[case] return_value: i64,
    ) {
        let mut test_phc_error_bound_file =
            NamedTempFile::new().expect("create mock phc error bound file failed");
        test_phc_error_bound_file
            .write_all(file_contents_to_write.as_bytes())
            .expect("write to mock phc error bound file failed");

        let phc_error_bound_reader =
            PhcWithSysfsErrorBound::new(test_phc_error_bound_file.path().to_path_buf(), 0);
        let rt = phc_error_bound_reader.read_phc_error_bound();
        assert!(rt.is_ok());
        assert_eq!(rt.unwrap(), return_value);
    }

    #[rstest]
    #[case::parsing_fail("asdf_not_an_i64")]
    fn test_read_phc_error_bound_bad_file_contents(#[case] file_contents_to_write: &str) {
        let mut test_phc_error_bound_file =
            NamedTempFile::new().expect("create mock phc error bound file failed");
        test_phc_error_bound_file
            .write_all(file_contents_to_write.as_bytes())
            .expect("write to mock phc error bound file failed");

        let phc_error_bound_reader =
            PhcWithSysfsErrorBound::new(test_phc_error_bound_file.path().to_path_buf(), 0);
        let rt = phc_error_bound_reader.read_phc_error_bound();
        assert!(rt.is_err());
        assert!(rt
            .unwrap_err()
            .to_string()
            .contains("Failed to parse PHC error bound value to i64"));
    }

    #[test]
    fn test_read_phc_error_bound_file_does_not_exist() {
        let phc_error_bound_reader = PhcWithSysfsErrorBound::new("/does/not/exist".into(), 0);
        let rt = phc_error_bound_reader.read_phc_error_bound();
        assert!(rt.is_err());
    }

    #[test]
    fn test_get_phc_ref_id() {
        let phc_error_bound_reader = PhcWithSysfsErrorBound::new("/does/not/matter".into(), 12345);
        assert_eq!(phc_error_bound_reader.get_phc_ref_id(), 12345);
    }

    #[test]
    fn test_get_error_bound_sysfs_path() {
        let ctx = mock_get_pci_slot::get_pci_slot_name_context();
        ctx.expect().returning(|_| Ok("12345".to_string()));
        let rt = get_error_bound_sysfs_path("arbitrary_interface");
        assert!(rt.is_ok());
        assert_eq!(
            rt.unwrap().to_str().unwrap(),
            "/sys/bus/pci/devices/12345/phc_error_bound"
        );
    }
}
