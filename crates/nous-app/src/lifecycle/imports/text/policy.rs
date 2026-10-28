use nous_domain::*;
use nous_foundation::configuration::{
    ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy,
};

pub const IMPORT_TEXT: ConfigKey<ImportTextSplitOptions> = ConfigKey::new("imports.text");

pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        IMPORT_TEXT,
        ConfigDefinition {
            owner: "imports".into(),
            description: "Text import strategy and source-preserving split windows.".into(),
            default: ImportTextSplitOptions::default(),
            scopes: vec![
                ConfigLevel::Instance,
                ConfigLevel::Namespace,
                ConfigLevel::Subject,
            ],
            apply_mode: ConfigApplyMode::Live,
            write_policy: ConfigWritePolicy::Scoped,
        },
        validate,
    )
}

fn validate(options: &ImportTextSplitOptions) -> Result<()> {
    if let Some(time) = &options.reference_time {
        time.validate()?;
    }
    Ok(())
}

pub(super) fn effective(
    mut options: ImportTextSplitOptions,
    overrides: Option<ImportTextSplitOverrides>,
) -> Result<ImportTextSplitOptions> {
    if let Some(value) = overrides {
        if let Some(strategy) = value.strategy {
            options.strategy = strategy;
        }
        if let Some(window) = value.narrative_window_size {
            options.narrative_window_size = window;
        }
        if let Some(overlap) = value.narrative_overlap {
            options.narrative_overlap = overlap;
        }
        if let Some(target) = value.factual_target_size {
            options.factual_target_size = target;
        }
        if value.reference_time.is_some() {
            options.reference_time = value.reference_time;
        }
    }
    options.narrative_window_size = options.narrative_window_size.max(200);
    options.narrative_overlap = options
        .narrative_overlap
        .min(options.narrative_window_size - 1);
    options.factual_target_size = options.factual_target_size.max(200);
    validate(&options)?;
    Ok(options)
}
