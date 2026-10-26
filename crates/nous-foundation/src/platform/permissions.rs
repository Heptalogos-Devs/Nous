//! Private instance data follows its filesystem owner across process identities.
use nous_domain::{Error, ErrorCode, Result};
use std::path::{Path, PathBuf};
#[derive(Debug, Clone)]
pub struct PrivatePermissions {
    root: PathBuf,
    #[cfg(windows)]
    owner_sid: String,
    #[cfg(unix)]
    uid: u32,
    #[cfg(unix)]
    gid: u32,
}
impl PrivatePermissions {
    pub fn for_instance(root: &Path) -> Result<Self> {
        let root = std::fs::canonicalize(root)?;
        let metadata = std::fs::metadata(&root)?;
        if !metadata.is_dir() {
            return Err(Error::invalid("instance root must be a directory"));
        }
        #[cfg(windows)]
        {
            use windows_permissions::{
                constants::{SeObjectType::SE_FILE_OBJECT, SecurityInformation},
                wrappers::GetNamedSecurityInfo,
            };
            let descriptor =
                GetNamedSecurityInfo(&root, SE_FILE_OBJECT, SecurityInformation::Owner)?;
            let owner_sid = descriptor
                .owner()
                .ok_or_else(|| Error::internal("instance filesystem owner is unavailable"))?
                .to_string();
            Ok(Self { root, owner_sid })
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                root,
                uid: metadata.uid(),
                gid: metadata.gid(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "private permissions are unavailable on this platform",
            ))
        }
    }
    pub fn protect_directory(&self, path: &Path) -> Result<()> {
        self.protect(path, true)
    }
    pub fn protect_root(&self) -> Result<()> {
        self.protect_directory(&self.root)
    }
    pub fn protect_file(&self, path: &Path) -> Result<()> {
        self.protect(path, false)
    }
    pub fn protect_private_tree(&self, path: &Path) -> Result<()> {
        let root = self.target(path, true)?;
        let mut pending = vec![root.clone()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(path) = pending.pop() {
            let target = match std::fs::canonicalize(&path) {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if !crate::instance::is_within(&target, &root) {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "private tree contains an external filesystem link",
                ));
            }
            if !seen.insert(target.clone()) {
                continue;
            }
            if std::fs::metadata(&target)?.is_dir() {
                self.protect_directory(&target)?;
                pending.extend(
                    std::fs::read_dir(target)?
                        .map(|entry| entry.map(|entry| entry.path()))
                        .collect::<std::io::Result<Vec<_>>>()?,
                );
            } else {
                self.protect_file(&target)?;
            }
        }
        Ok(())
    }
    fn target(&self, path: &Path, directory: bool) -> Result<PathBuf> {
        let target = std::fs::canonicalize(path)?;
        if !crate::instance::is_within(&target, &self.root) {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "private path escapes its instance",
            ));
        }
        let metadata = std::fs::metadata(&target)?;
        if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(Error::invalid(
                "private path has an unexpected filesystem type",
            ));
        }
        Ok(target)
    }
    #[cfg(windows)]
    pub fn owner_sid(&self) -> &str {
        &self.owner_sid
    }
    #[cfg(windows)]
    fn protect(&self, path: &Path, directory: bool) -> Result<()> {
        use windows_permissions::constants::{SeObjectType::SE_FILE_OBJECT, SecurityInformation};
        use windows_permissions::{LocalBox, SecurityDescriptor, wrappers::SetNamedSecurityInfo};
        let target = self.target(path, directory)?;
        let inheritance = if directory { "OICI" } else { "" };
        let descriptor: LocalBox<SecurityDescriptor> = format!(
            "D:P(A;{inheritance};FA;;;{})(A;{inheritance};FA;;;SY)",
            self.owner_sid
        )
        .parse()?;
        let dacl = descriptor
            .dacl()
            .ok_or_else(|| Error::internal("private ACL descriptor has no DACL"))?;
        SetNamedSecurityInfo(
            &target,
            SE_FILE_OBJECT,
            SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
            None,
            None,
            Some(dacl),
            None,
        )?;
        Ok(())
    }
    #[cfg(unix)]
    pub fn owner_ids(&self) -> (u32, u32) {
        (self.uid, self.gid)
    }
    #[cfg(unix)]
    pub fn require_owner_process(&self) -> Result<()> {
        if uzers::get_effective_uid() != self.uid {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "user service must run as the instance filesystem owner",
            ));
        }
        Ok(())
    }
    #[cfg(unix)]
    pub fn owner_name(&self) -> Result<String> {
        let user = uzers::get_user_by_uid(self.uid).ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "instance filesystem owner has no OS user account",
            )
        })?;
        let name = user
            .name()
            .to_str()
            .ok_or_else(|| Error::invalid("instance owner account name is not UTF-8"))?;
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(Error::invalid("instance owner account name is invalid"));
        }
        Ok(name.to_owned())
    }
    #[cfg(unix)]
    fn protect(&self, path: &Path, directory: bool) -> Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
        let target = self.target(path, directory)?;
        if std::fs::metadata(&target)?.uid() != self.uid {
            chown(&target, Some(self.uid), None)?;
        }
        std::fs::set_permissions(
            target,
            std::fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )?;
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    fn protect(&self, _path: &Path, _directory: bool) -> Result<()> {
        Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "private permissions are unavailable on this platform",
        ))
    }
}
