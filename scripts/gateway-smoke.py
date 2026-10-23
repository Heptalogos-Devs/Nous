"""Run the complete current cognition flow using a read-only real gateway template.

Usage: python scripts/gateway-smoke.py <binary> --template <configured-instance>
This optional entrypoint makes real model calls in a new isolated instance. It
forms two source Memories, six structured kinds in two committed stages, then
uses real cognition embeddings and tests cached retrieval after restarting.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import sys
import tomllib
sys.dont_write_bytecode = True
from smoke_runtime import Runner, Mcp, prepare_template, require


def reference(value):
    return {"kind": value["body"]["kind"], "id": value["id"], "revision": value["revision"]}


def exercise(runner, instance):
    base = runner.start(instance)
    namespace = runner.cli(instance, "namespace", "create", "Gateway field research")["result"]["id"]
    subject = runner.cli(instance, "subject", "create", "--namespace", namespace, "--name", "field-reader")["result"]["id"]
    texts = [
        "独立现场记录 NW-ST-7421-A：海岬研究组的林岚在2026年9月1日进行海水采样，发现当天的温敏藻类样本必须放入2至8摄氏度的蓝色保温箱再运输，否则检验活性下降。该结论只针对本次温敏样本和所述运输条件，未覆盖耐热样本。作者林岚称这个操作流程为冷链采样。",
        "独立现场记录 NW-ST-7421-B：岛礁实验室的陈洛在2026年9月15日另行开展海水采样试验。她记录该批温敏藻类样本使用2至8摄氏度保温箱运输后仍有可检测活性，未保温的对照活性下降。实验只覆盖该批样本和两小时运输，不推出所有海洋样本都需要冷链。作者陈洛也将这个流程称作低温采样运输。",
    ]
    materials, observations, memories, receipts = [], [], [], []
    producer_calls = set()
    runner.report.update(namespace_id=namespace, subject_id=subject, job_ids=[], formation_outcomes=[])
    for index, text in enumerate(texts):
        path = runner.runtime / f"source-{index}.txt"
        path.write_text(text, encoding="utf-8")
        material = runner.cli(instance, "material", "put", "--namespace", namespace, "--file", str(path), "--source", f"gateway:independent-{index}")["result"]
        observation = runner.cli(instance, "observation", "record", "--subject", subject, "--material", material["id"])["result"]
        require(observation["kind"] == "contact", "explicit reading is not an actual contact")
        receipt = runner.cli(instance, "memory", "form", "--subject", subject, "--observation", observation["id"], "--operation-id", f"memory-{index}")
        job = receipt["job_ids"][0]
        runner.report["job_ids"].append(job)
        completed = runner.wait_job(instance, base, job)
        ids = completed["job"]["result"]["memory_ids"]
        require(ids, "completed Memory formation produced no memory")
        memory = runner.cli(instance, "memory", "get", ids[0])
        source = runner.cli(instance, "memory", "source", memory["id"])
        require(source["material"]["text"] == text and source["observation"]["id"] == observation["id"], "formed Memory lost its actual source")
        require(source["model_call"]["state"] == "succeeded", "Memory has no successful actual model call")
        producer_calls.add(source["model_call"]["id"])
        materials.append(material["id"])
        observations.append(observation["id"])
        memories.append(memory)
        receipts.append(receipt)
    runner.report["checks"].update(real_memory_formation=True, independent_materials=True, contact_sources=True)

    def cognition(action, **fields):
        return runner.http(instance, base, "/api/cognition", dict(action=action, **fields))[1]

    def form(request, name):
        receipt = runner.http(instance, base, "/api/cognition", request)[1]
        job = receipt["job_ids"][0]
        runner.report["job_ids"].append(job)
        completed = runner.wait_job(instance, base, job)
        producer_calls.update(call["id"] for call in completed["model_calls"] if call["state"] == "succeeded")
        state = cognition("formation_run", job_id=job)
        runner.record(name + ".json", state)
        runner.report["formation_outcomes"].append({"job_id": job, "groups": [
            {"key": group["key"], "state": group["state"], "error": group.get("error")} for group in state["groups"]]})
        objects = [item for group in state["groups"] if group["state"] == "applied" for item in group["result"]["objects"]]
        return receipt, state, objects

    sources = [{"kind": "observation", "observation_id": value} for value in observations]
    sources += [{"kind": "memory_revision", "memory_id": value["id"], "revision": value["revision"]} for value in memories]
    targets = [("source_a_semantics", "memory_semantics"), ("source_b_semantics", "memory_semantics"),
               ("sampling_concept", "concept"), ("sampling_episode", "episode"), ("bounded_schema", "schema"),
               ("concept_episode_association", "association"), ("concept_schema_association", "association")]
    first_request = {"action": "form", "operation_id": "grounded-structured", "input": {
        "subject_id": subject, "sources": sources,
        "targets": [{"key": key, "kind": kind, "existing": None} for key, kind in targets]}}
    first_receipt, first_state, objects = form(first_request, "grounded-structured")
    requests, formed_states = [(first_request, first_receipt)], [first_state]
    incomplete = []
    episode = next((value for value in objects if value["body"]["kind"] == "episode"), None)
    if episode:
        episode_source = cognition("source", id=episode["id"], revision=episode["revision"])
        related = {root["material_id"] for root in episode_source["closure"]["roots"]}
        journal_request = {"action": "form", "operation_id": "journal-after-episode", "input": {
            "subject_id": subject,
            "sources": [{"kind": "cognitive_revision", "reference": reference(episode)}] + [
                {"kind": "observation", "observation_id": observation} for material, observation in zip(materials, observations) if material in related],
            "targets": [{"key": "committed_episode_journal", "kind": "journal", "existing": None}]}}
        receipt, state, journals = form(journal_request, "journal-after-episode")
        requests.append((journal_request, receipt))
        formed_states.append(state)
        objects.extend(journals)
        for item in journals:
            if item["body"]["kind"] == "journal":
                require(item["body"]["content"]["knowledge_cutoff"] >= episode["commit_seq"], "Journal predates its accepted Episode")
                if reference(episode) not in item["body"]["content"]["episodes"]:
                    incomplete.append("journal_references_committed_episode")
    else:
        runner.report["journal"] = "not_scheduled_missing_episode"
    produced = {item["body"]["kind"] for item in objects}
    runner.report["produced_kinds"] = sorted(produced)
    required = {"memory_semantics", "concept", "episode", "schema", "association", "journal"}
    missing = sorted(required - produced) + incomplete
    semantics = {item["body"]["content"]["memory_id"] for item in objects if item["body"]["kind"] == "memory_semantics"}
    if semantics != {memory["id"] for memory in memories}:
        missing.append("semantics_for_both_memories")
    source_snapshots = {}
    for item in objects:
        source = cognition("source", id=item["id"], revision=item["revision"])
        roots = {root["material_id"] for root in source["closure"]["roots"]}
        require(source["closure"]["complete"] and roots and roots <= set(materials), "structured object has invented or incomplete roots")
        require({value["material"]["id"] for value in source["materials"]} == roots, "structured source omitted original materials")
        require(source["producer"]["id"] == item["producer_call_id"], "structured object lost the actual producer")
        if item["body"]["kind"] == "schema" and len(roots) != 2:
            missing.append("schema_with_two_independent_roots")
        source_snapshots[item["id"]] = source
        producer_calls.add(source["producer"]["id"])
    runner.record("structured-sources.json", source_snapshots)
    runner.report["structured_incomplete"] = missing
    runner.report["checks"].update(fixed_target_slots=True, independent_group_outcomes=True, source_closure=True)

    embedding = runner.cli(instance, "memory", "embed", "--subject", subject, "--operation-id", "all-cognition-vectors")
    for job in embedding["result"]["jobs"]:
        runner.report["job_ids"].append(job)
        runner.wait_job(instance, base, job)
    concept = next((item for item in objects if item["body"]["kind"] == "concept"), None)
    selected = reference(concept) if concept else {"kind": "memory", "id": memories[0]["id"], "revision": memories[0]["revision"]}
    text = concept["body"]["content"]["label"] if concept else memories[0]["title"]
    query = {"subject_id": subject, "text": text, "kinds": [selected["kind"]], "mode": "dense"}
    first = runner.http(instance, base, "/api/query", query)[1]
    require(selected in [hit["reference"] for hit in first["hits"]], "real dense query omitted its formed cognition")
    semantic = first["semantic"]
    provenance = [item for item in semantic["provenance"] if item["target"].get("reference") == selected]
    require(provenance, "query has no typed accepted-vector provenance")
    vector_call = runner.cli(instance, "model", "call", provenance[0]["model_call_id"])
    config = tomllib.loads((instance / "instance.toml").read_text(encoding="utf-8"))
    embedding_config = config["models"]["text_embedding"]
    require(vector_call["response"]["record"]["provider_model"] == embedding_config["expected_provider_model"], "actual embedding model differs from configured space")
    require(vector_call["request"]["space"]["dimension"] == embedding_config["dimension"], "actual embedding dimension differs")
    producer_calls.update((vector_call["id"], semantic["query_embedding_call_id"]))
    before = runner.call_count(instance)
    for mode in ("dense", "vcp", "hybrid"):
        result = runner.http(instance, base, "/api/query", dict(query, mode=mode))[1]
        require(selected in [hit["reference"] for hit in result["hits"]], f"{mode} omitted current cognition")
        require(result["semantic"]["query_embedding_call_id"] == semantic["query_embedding_call_id"], "query vector cache changed the actual call")
    require(runner.call_count(instance) == before, "cached query modes scheduled another model attempt")
    for request, accepted in requests:
        require(runner.http(instance, base, "/api/cognition", request)[1] == accepted, "formation operation did not replay")
    for index, observation in enumerate(observations):
        require(runner.cli(instance, "memory", "form", "--subject", subject, "--observation", observation, "--operation-id", f"memory-{index}") == receipts[index], "Memory formation receipt did not replay")
    for stdio in (False, True):
        service = Mcp(runner, instance, base, stdio=stdio)
        try:
            names = {tool["name"] for tool in service.send("tools/list")["tools"]}
            require({"cognition", "query", "model_call"} <= names, "MCP omits current cognition operations")
            require(selected in [hit["reference"] for hit in service.tool("query", query)["hits"]], "MCP query selected another object")
            if concept:
                source = service.tool("cognition", {"action": "source", "id": concept["id"], "revision": concept["revision"]})
                require(source == source_snapshots[concept["id"]], "MCP changed the exact structured source")
        finally:
            service.close()
    require(runner.call_count(instance) == before, "read/replay checks scheduled another model call")
    for memory in memories:
        producer_calls.add(runner.cli(instance, "memory", "source", memory["id"])["model_call"]["id"])
    calls = []
    for identity in sorted(producer_calls):
        call = runner.cli(instance, "model", "call", identity)
        record, request = call["response"]["record"], call["request"]
        require(call["state"] == "succeeded" and record["provider_model"] and record["usage"], "actual producer evidence is missing")
        if "prompt" in request:
            prompt = request["prompt"]
            require(hashlib.sha256(prompt["text"].encode()).hexdigest() == prompt["sha256"], "rendered prompt hash differs")
            require(hashlib.sha256(prompt["template"].encode()).hexdigest() == prompt["template_sha256"], "actual template hash differs")
        if "output_schema" in request:
            encoded = json.dumps(request["output_schema"], ensure_ascii=False, separators=(",", ":")).encode()
            require(hashlib.sha256(encoded).hexdigest() == request["output_schema_sha256"], "actual output schema hash differs")
        runner.record("model-call-" + identity + ".json", call)
        calls.append({"id": identity, "role": request["role"], "provider_model": record["provider_model"], "usage": record["usage"]})
    runner.report["calls"] = calls
    runner.report["checks"].update(real_cognition_embeddings=True, dense=True, vcp=True, hybrid=True, persistent_query_cache=True,
                                    shared_model_ledger=True, actual_prompt_hashes=True, http_mcp=True, stdio_mcp=True)
    runner.stop(instance)
    for relative in {role["resource"]["credential"]["file"] for role in config["models"].values()}:
        path = (instance / relative).resolve(strict=True)
        require(path.is_relative_to((instance / "credentials").resolve()), "owned credential reference escaped instance")
        path.unlink()
    base = runner.start(instance)
    recovered = runner.http(instance, base, "/api/query", dict(query, mode="vcp"))[1]
    require(selected in [hit["reference"] for hit in recovered["hits"]], "restart lost accepted semantic cognition")
    require(recovered["semantic"]["query_embedding_call_id"] == semantic["query_embedding_call_id"], "restart changed cached query producer")
    for item in objects:
        require(cognition("get", id=item["id"], revision=item["revision"]) == item, "restart changed exact cognition")
        require(cognition("source", id=item["id"], revision=item["revision"]) == source_snapshots[item["id"]], "restart changed source closure")
    for previous in formed_states:
        job = previous["run"]["job"]["id"]
        require(cognition("formation_run", job_id=job)["groups"] == previous["groups"], "restart changed independent group outcomes")
    require(runner.call_count(instance) == before, "restart needed another provider attempt")
    runner.stop(instance)
    runner.report["checks"].update(restart_cognition=True, restart_sources=True, offline_cached_query=True)
    runner.report.update(model_calls=before, object_references=[reference(value) for value in objects])
    require(not missing, f"structured proposal outcomes left required capabilities incomplete: {missing}")
    runner.report["checks"]["complete_structured_chain"] = True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--template", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=300)
    args = parser.parse_args()
    runner = Runner(args.binary.resolve(strict=True), args.timeout, label="gateway-smoke")
    failure, instance = None, None
    try:
        instance = prepare_template(runner, args.template.resolve(strict=True))
        exercise(runner, instance)
    except Exception as error:
        failure = error
        if instance is not None:
            observed = {call["id"]: call for call in runner.report.get("calls", [])}
            for job in runner.report.get("job_ids", []):
                try:
                    snapshot = runner.cli(instance, "job", "get", job)
                    runner.record("job-" + job + ".json", snapshot)
                    for call in snapshot["model_calls"]:
                        record = (call["response"] or {}).get("record", {})
                        observed[call["id"]] = {"id": call["id"], "state": call["state"], "role": call["request"].get("role"),
                                                 "provider_model": record.get("provider_model"), "usage": record.get("usage")}
                except Exception:
                    pass
            runner.report["calls"] = list(observed.values())
            try:
                runner.report["model_calls"] = runner.call_count(instance)
            except Exception:
                pass
    finally:
        runner.close()
        report = runner.save(failure)
    print(json.dumps({"passed": failure is None, "report": str(report)}, ensure_ascii=False))
    if failure:
        raise failure


if __name__ == "__main__":
    main()
