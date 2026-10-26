use super::{Media, PreparationRequest};
use nous_domain::{
    ErrorCode, Result,
    media::{MediaDelivery, MediaMetadata, SourceCoordinate},
};

impl Media {
    /// Inspect required registered resources without reading source bytes or executing tools.
    pub async fn preparation_ready(&self, request: &PreparationRequest) -> Result<bool> {
        request.validate()?;
        let policy = self.policy().await?;
        let resources = if matches!(request.delivery, MediaDelivery::SampledFrames) {
            vec![
                policy.ffmpeg_resource.as_str(),
                policy.probe_resource.as_str(),
            ]
        } else {
            match &request.coordinate {
                SourceCoordinate::Whole
                    if !matches!(request.metadata, MediaMetadata::Image { frames: 2.., .. }) =>
                {
                    Vec::new()
                }
                SourceCoordinate::TextSpan { .. } => Vec::new(),
                SourceCoordinate::ByteRange { .. }
                    if matches!(request.metadata, MediaMetadata::Text) =>
                {
                    Vec::new()
                }
                SourceCoordinate::ByteRange { .. } => vec![policy.probe_resource.as_str()],
                SourceCoordinate::Page { .. } => {
                    let Some(resource) = policy.pdf_resource.as_deref() else {
                        return Ok(false);
                    };
                    vec![resource, policy.probe_resource.as_str()]
                }
                _ => vec![
                    policy.ffmpeg_resource.as_str(),
                    policy.probe_resource.as_str(),
                ],
            }
        };
        for resource in resources {
            match self.platform.executable(resource) {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::CapabilityUnavailable | ErrorCode::DependencyUnavailable
                    ) =>
                {
                    return Ok(false);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}
