// mutool info/draw options verified against ArtifexSoftware/mupdf a9642221980b092bb60b2e2ef79aa2e97bfb12eb.
use super::{Media, MediaPolicy, PreparationRequest, PreparedItem};
use crate::platform::{ProcessSpec, StopReason};
use nous_domain::{
    CallContext, Error, ErrorCode, Result,
    media::{BlobReference, MediaMetadata, SourceCoordinate},
};
use std::{ffi::OsString, time::Duration};
use tokio_util::sync::CancellationToken;

impl Media {
    pub(super) async fn document_metadata(
        &self,
        ctx: &CallContext,
        blob: &BlobReference,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<MediaMetadata> {
        let Some(resource) = &policy.pdf_resource else {
            return Ok(MediaMetadata::Document { pages: None });
        };
        let workspace = self.platform.workspace()?;
        let input = workspace.path().join("source.pdf");
        self.objects
            .copy_to(&blob.content_hash, blob.byte_len as i64, &input, cancel)
            .await?;
        let mut spec = ProcessSpec::new(resource);
        spec.timeout = Some(Duration::from_millis(policy.probe_timeout_ms));
        spec.args = vec!["info".into(), input.into_os_string()];
        let output = self
            .platform
            .start(ctx, workspace, spec, cancel.clone())
            .await?
            .wait()
            .await?;
        if !output.exit.success || !matches!(output.reason, StopReason::Exited) {
            return Err(Error::new(
                ErrorCode::DependencyUnavailable,
                "PDF metadata probing failed",
            ));
        }
        let text = std::str::from_utf8(&output.stdout)
            .map_err(|error| Error::invalid("PDF info is not UTF-8").with_cause(&error))?;
        let pages = text
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("Pages:")?
                    .trim()
                    .parse::<u32>()
                    .ok()
            })
            .filter(|pages| *pages > 0)
            .ok_or_else(|| Error::invalid("PDF probe did not report a positive page count"))?;
        Ok(MediaMetadata::Document { pages: Some(pages) })
    }
    pub(super) async fn render_page(
        &self,
        ctx: &CallContext,
        request: &PreparationRequest,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<PreparedItem> {
        let resource = policy.pdf_resource.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "PDF page preparation requires a registered mutool resource",
            )
        })?;
        let SourceCoordinate::Page { number } = request.coordinate else {
            return Err(Error::invalid("document renderer requires a page locator"));
        };
        let (workspace, input) = self.stage(request, cancel).await?;
        let output = workspace.path().join("page.png");
        let mut spec = ProcessSpec::new(resource);
        spec.timeout = Some(Duration::from_millis(policy.transform_timeout_ms));
        spec.args = [
            OsString::from("draw"),
            "-F".into(),
            "png".into(),
            "-w".into(),
            policy.frame_max_dimension.to_string().into(),
            "-h".into(),
            policy.frame_max_dimension.to_string().into(),
            "-o".into(),
            output.clone().into_os_string(),
            input.into_os_string(),
            number.to_string().into(),
        ]
        .into();
        let result = self
            .platform
            .start(ctx, workspace, spec, cancel.clone())
            .await?
            .wait()
            .await?;
        if !result.exit.success || !matches!(result.reason, StopReason::Exited) {
            return Err(Error::new(
                ErrorCode::DependencyUnavailable,
                "PDF page rendering failed",
            ));
        }
        let item = self
            .publish_output(
                ctx,
                &output,
                request.coordinate.clone(),
                None,
                policy,
                cancel,
            )
            .await?;
        drop(result);
        Ok(item)
    }
    pub(super) async fn document_version(
        &self,
        ctx: &CallContext,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let resource = policy.pdf_resource.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "PDF renderer is not configured",
            )
        })?;
        let mut spec = ProcessSpec::new(resource);
        spec.args = vec!["-v".into()];
        let result = self.platform.run(ctx, spec, cancel.clone()).await?;
        if !result.exit.success || !matches!(result.reason, StopReason::Exited) {
            return Err(Error::new(
                ErrorCode::DependencyUnavailable,
                "PDF renderer version query failed",
            ));
        }
        let text = if result.stdout.is_empty() {
            &result.stderr
        } else {
            &result.stdout
        };
        let version = String::from_utf8_lossy(text)
            .lines()
            .next()
            .filter(|line| line.contains("mutool") || line.contains("mupdf"))
            .map(str::to_owned)
            .ok_or_else(|| Error::invalid("registered PDF resource did not identify mutool"))?;
        Ok(version)
    }
}
