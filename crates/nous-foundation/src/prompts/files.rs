use super::*;
use crate::instance::InstancePaths;
use nous_domain::{Error, ErrorCode, Result};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

pub(super) fn template(
    paths: &InstancePaths,
    file: &Path,
    locale: Option<String>,
) -> Result<PromptTemplate> {
    let binding = PromptBinding::File {
        file: file.into(),
        locale: locale.clone(),
    };
    binding.validate()?;
    let path = managed_path(paths, file)?;
    let file_handle = File::open(path).map_err(Error::dependency)?;
    if !file_handle.metadata().map_err(Error::dependency)?.is_file() {
        return Err(Error::invalid("prompt must be a regular file"));
    }
    let mut text = String::new();
    file_handle
        .take((MAX_TEMPLATE_BYTES + 1) as u64)
        .read_to_string(&mut text)
        .map_err(Error::dependency)?;
    let id = file.to_string_lossy().replace('\\', "/");
    let variables = validate_template(&id, &text)?;
    Ok(PromptTemplate {
        id,
        binding,
        locale,
        sha256: digest(&text),
        text,
        variables,
    })
}
pub(super) fn reference(
    paths: &InstancePaths,
    binding: &PromptBinding,
) -> Result<Option<ManagedPromptReference>> {
    let PromptBinding::File { file, locale } = binding else {
        return Ok(None);
    };
    let asset = template(paths, file, locale.clone())?;
    Ok(Some(ManagedPromptReference {
        relative_path: file.clone(),
        path: managed_path(paths, file)?,
        sha256: asset.sha256,
        byte_len: asset.text.len() as u64,
    }))
}
fn managed_path(paths: &InstancePaths, file: &Path) -> Result<PathBuf> {
    let path = paths
        .resolve(file)
        .map_err(|error| Error::invalid(error.to_string()))?;
    if !path
        .canonicalize()
        .map_err(Error::dependency)?
        .starts_with(paths.prompts.canonicalize().map_err(Error::dependency)?)
    {
        return Err(Error::new(
            ErrorCode::Forbidden,
            "prompt file is outside the instance prompts directory",
        ));
    }
    Ok(path)
}
pub(super) fn catalog(
    paths: &InstancePaths,
    limit: usize,
) -> Result<(Vec<PromptCatalogEntry>, bool)> {
    let mut pending = vec![(paths.prompts.clone(), 0)];
    let mut files = Vec::new();
    while let Some((directory, depth)) = pending.pop() {
        if depth > 8 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "prompt directory exceeds eight levels",
            ));
        }
        for entry in fs::read_dir(directory).map_err(Error::dependency)? {
            let entry = entry.map_err(Error::dependency)?;
            let kind = entry.file_type().map_err(Error::dependency)?;
            if kind.is_dir() {
                pending.push((entry.path(), depth + 1));
            } else if kind.is_file() {
                files.push(entry.path());
                if files.len() > limit {
                    return Ok((describe(paths, &files[..limit]), false));
                }
            }
        }
    }
    files.sort();
    Ok((describe(paths, &files), true))
}
fn describe(paths: &InstancePaths, files: &[PathBuf]) -> Vec<PromptCatalogEntry> {
    files
        .iter()
        .map(|path| {
            let file = path
                .strip_prefix(&paths.root)
                .expect("catalog walks the instance prompts directory")
                .to_owned();
            let id = file.to_string_lossy().replace('\\', "/");
            match template(paths, &file, None) {
                Ok(asset) => PromptCatalogEntry {
                    id,
                    binding: asset.binding,
                    locale: asset.locale,
                    byte_len: Some(asset.text.len()),
                    variables: asset.variables,
                    error: None,
                },
                Err(error) => PromptCatalogEntry {
                    id,
                    binding: PromptBinding::File { file, locale: None },
                    locale: None,
                    byte_len: None,
                    variables: vec![],
                    error: Some(error),
                },
            }
        })
        .collect()
}
