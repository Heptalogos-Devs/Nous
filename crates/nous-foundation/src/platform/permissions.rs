//! Fixed owner-only credential permissions through native platform ACLs.
use nous_domain::Result;
use std::path::Path;

pub fn protect_directory(path: &Path) -> Result<()> {
    protect(path, true)
}

pub fn protect_file(path: &Path) -> Result<()> {
    protect(path, false)
}

#[cfg(unix)]
fn protect(path: &Path, directory: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
    )?;
    Ok(())
}

#[cfg(windows)]
fn protect(path: &Path, directory: bool) -> Result<()> {
    use windows_permissions::constants::{SeObjectType::SE_FILE_OBJECT, SecurityInformation};
    use windows_permissions::{
        LocalBox, SecurityDescriptor, utilities::current_process_sid,
        wrappers::SetNamedSecurityInfo,
    };
    let sid = current_process_sid()?;
    let inheritance = if directory { "OICI" } else { "" };
    let descriptor: LocalBox<SecurityDescriptor> =
        format!("D:P(A;{inheritance};FA;;;{sid})").parse()?;
    let dacl = descriptor
        .dacl()
        .ok_or_else(|| nous_domain::Error::internal("credential ACL descriptor has no DACL"))?;
    SetNamedSecurityInfo(
        path,
        SE_FILE_OBJECT,
        SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
        None,
        None,
        Some(dacl),
        None,
    )?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn protect(_path: &Path, _directory: bool) -> Result<()> {
    Err(nous_domain::Error::new(
        nous_domain::ErrorCode::CapabilityUnavailable,
        "credential permissions are unavailable on this platform",
    ))
}
