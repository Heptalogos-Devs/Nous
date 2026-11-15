use serde_json::json;
use std::io::Write;

fn publish(path: &std::path::Path, bytes: &[u8]) {
    if std::fs::read(path).is_ok_and(|current| current == bytes) {
        return;
    }
    let mut pending = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
    pending.write_all(bytes).unwrap();
    pending.persist(path).unwrap();
}
fn main() {
    let resources = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../../src/nous_native/resources");
    std::fs::create_dir_all(&resources).unwrap();
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let schema = json!({"build":schemars::schema_for!(nous_vcp::config::BuildConfig),"query":schemars::schema_for!(nous_vcp::config::QueryControls),"context":schemars::schema_for!(nous_vcp::config::ContextControls),"pipeline":schemars::schema_for!(nous_vcp::pipeline::PipelineConfig),"readout":schemars::schema_for!(nous_vcp::river::NativeConfig),"dtsc":schemars::schema_for!(nous_vcp::dtsc::ReferenceDtscConfig)});
    let mut schema = schema;
    schema["operators"] = json!({
        "vcp.observe": schemars::schema_for!(nous_vcp::config::EmptyOperatorParameters),
        "ann.search": schemars::schema_for!(nous_vcp::config::AnnParameters),
        "lexical.search": schemars::schema_for!(nous_vcp::config::LaneParameters),
        "concept.direct": schemars::schema_for!(nous_vcp::config::LaneParameters),
        "candidates.union": schemars::schema_for!(nous_vcp::config::UnionParameters),
        "vcp.calibrate": schemars::schema_for!(nous_vcp::config::EmptyOperatorParameters),
        "river.readout": schemars::schema_for!(nous_vcp::config::ReadoutParameters),
        "dtsc.readout": schemars::schema_for!(nous_vcp::config::ReadoutParameters),
        "vcp.think": schemars::schema_for!(nous_vcp::config::EmptyOperatorParameters),
        "rank.from_lane": schemars::schema_for!(nous_vcp::config::LaneRankParameters),
        "rank.cosine": schemars::schema_for!(nous_vcp::config::EmptyOperatorParameters),
        "rank.fuse": schemars::schema_for!(nous_vcp::config::FusionParameters),
        "rank.context": schemars::schema_for!(nous_vcp::config::EmptyOperatorParameters),
        "context.recall": schemars::schema_for!(nous_vcp::config::EmptyOperatorParameters),
        "passages.locate": schemars::schema_for!(nous_vcp::config::LocateParameters),
        "passages.select": schemars::schema_for!(nous_vcp::config::SelectParameters),
        "delivery.read": schemars::schema_for!(nous_vcp::config::EmptyOperatorParameters),
    });
    let defaults = json!({"build":nous_vcp::config::BuildConfig::default(),"query":nous_vcp::config::QueryControls::default(),"context":nous_vcp::config::ContextControls::default(),"pipeline":nous_vcp::pipeline::PipelineConfig::default(),"readout":nous_vcp::river::NativeConfig::default(),"dtsc":nous_vcp::dtsc::ReferenceDtscConfig::default()});
    for (name, value) in [("schemas.json", schema), ("defaults.json", defaults)] {
        let bytes = serde_json::to_vec_pretty(&value).unwrap();
        publish(&out.join(name), &bytes);
        publish(&resources.join(name), &bytes);
    }
    println!("cargo:rerun-if-changed=../vcp/src");
    println!("cargo:rerun-if-changed=build.rs");
}
