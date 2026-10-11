// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{VcpQueryObservation, VcpServingGeneration};
use nous_core::{Error, Result};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn explain_routes(
    generation: &VcpServingGeneration,
    observation: &VcpQueryObservation,
    ranked: &mut [(i64, Value)],
) -> Result<()> {
    let sense = &observation.numerical().sense;
    let routes = crate::activated_routes::activated_routes(
        observation.seed_ids(),
        sense
            .edges
            .iter()
            .filter(|edge| edge.flow > 0.0)
            .map(|edge| (edge.source_id, edge.target_id)),
        observation.policy().sense.max_safe_hops,
    );
    let activated: BTreeMap<_, _> = sense
        .edges
        .iter()
        .map(|edge| ((edge.source_id, edge.target_id), edge))
        .collect();
    let mut basis = BTreeMap::<_, Vec<Value>>::new();
    for evidence in &generation.graph.evidence {
        basis.entry((evidence.source_id,evidence.target_id)).or_default().push(json!({
            "provenance_root": evidence.provenance_root, "basis_class": evidence.basis_class,
            "association_kind": evidence.association_kind, "support_mass": evidence.support_mass,
            "polarity": "positive"
        }));
    }
    let document_roots: BTreeMap<_, _> = generation
        .graph
        .membership
        .iter()
        .map(|file| {
            Ok((
                generation.identities.reference(file.file_id)?.to_string(),
                file.file_id,
            ))
        })
        .collect::<Result<_>>()?;
    // File-fact roots denote actual document membership cooccurrence. Preserve
    // that ontology; they are not automatically learned Association objects.
    for edge in &generation.graph.graph.provenance {
        for (root_id, mass) in &edge.file_contributions {
            let index = usize::try_from(*root_id - 1)
                .map_err(|_| Error::Invalid("VCP root ID invalid".into()))?;
            let root = generation
                .graph
                .provenance_roots
                .get(index)
                .ok_or_else(|| Error::Invalid("VCP root ID outside asset".into()))?;
            if *mass > 0.0 && document_roots.contains_key(root) {
                basis.entry((edge.source_id,edge.target_id)).or_default().push(json!({
                    "provenance_root": root,"association_kind": "vcp.document_cooccurrence",
                    "basis_class": "derived_structure","support_mass": mass,"polarity": "positive"
                }));
            }
        }
    }
    for (id, metadata) in ranked {
        let object = metadata
            .as_object_mut()
            .ok_or_else(|| Error::Invalid("VCP readout metadata must be an object".into()))?;
        let path = routes.get(id);
        let references = path
            .map(|path| {
                path.iter()
                    .map(|id| {
                        generation
                            .identities
                            .reference(*id)
                            .map(ToString::to_string)
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let edges = path
            .map(|path| {
                path.windows(2)
                    .map(|pair| {
                        let key = (pair[0], pair[1]);
                        let evidence = basis.get(&key);
                        Ok(json!({ "from": generation.identities.reference(pair[0])?,
                "to": generation.identities.reference(pair[1])?,
                "flow": activated[&key].flow, "wormhole": activated[&key].wormhole,
                "support": evidence.into_iter().flatten().take(16).collect::<Vec<_>>(),
                "support_truncated": evidence.is_some_and(|values| values.len() > 16) }))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        object.insert("activated_route".into(), json!(references));
        object.insert("route_evidence".into(), json!(edges));
        object.insert(
            "route_seed".into(),
            json!(
                path.and_then(|path| path.first())
                    .and_then(|id| observation.route_seed(*id))
            ),
        );
        object.insert(
            "route_hop_limit".into(),
            json!(observation.policy().sense.max_safe_hops.min(32)),
        );
        object.insert("route_semantics".into(),json!("shortest witnessed Sense route; not score attribution; unsupported semantic transitions carry no cognitive evidence"));
    }
    Ok(())
}
