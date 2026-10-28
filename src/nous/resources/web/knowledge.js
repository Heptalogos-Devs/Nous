import {
  $,
  state,
  el,
  api,
  subjectPath,
  op,
  table,
  button,
  form,
  notice,
  error,
  busy,
  jsonView,
  chip,
  download,
} from "./client.js";
const kindNames = {
  memory: "记忆",
  memory_semantics: "记忆语义",
  concept: "概念",
  entity: "实体",
  episode: "情节",
  journal: "日记",
  schema: "图式",
  association: "关联",
  self_facet: "自我",
  narrative_thread: "叙事线",
  domain_knowledge: "领域知识",
  social_relation: "社会关系",
  language_convention: "语言惯例",
  desire: "愿望",
  concern: "关切",
  material: "原件",
  source_region: "来源区域",
};
const exact = (ref) =>
  ref.kind === "material" || ref.kind === "source_region"
    ? `${ref.kind.replaceAll("_", "-")}:${ref.id}`
    : `${ref.kind}:${ref.id}@${ref.revision}`;
const bodyText = (item) => {
  const b = item.body;
  return (
    b?.text || b?.narrative || b?.description || b?.claim || b?.understanding || item.filename || ""
  );
};
function title(item) {
  return (
    item.body?.title ||
    item.body?.label ||
    item.body?.expression ||
    item.filename ||
    bodyText(item).slice(0, 70) ||
    exact(item.reference)
  );
}
export async function query() {
  const domain = $("domain").value;
  const result = await api(subjectPath("/query"), {
    method: "POST",
    body: {
      text: $("query").value,
      mode: $("mode").value,
      domains: domain === "both" ? ["cognition", "media"] : [domain],
      limit: 30,
    },
  });
  $("result-count").textContent = `${result.hits.length} 条结果`;
  $("coverage").textContent = result.coverage.complete ? "索引覆盖完整" : "索引尚未完整覆盖";
  const list = $("results");
  list.replaceChildren();
  if (!result.hits.length) {
    const empty = el("div", "empty");
    empty.append(
      el("h2", "", "没有匹配的认知"),
      el("p", "", "可以调整查询、返回域，或接纳新资料。"),
    );
    list.append(empty);
    return;
  }
  for (const hit of result.hits) {
    const item = hit.object;
    const row = el("button", "result-row");
    const meta = el("div", "row-meta");
    meta.append(
      el("span", "", kindNames[item.reference.kind] || item.reference.kind),
      el("span", "", Number(hit.score).toFixed(3)),
    );
    row.append(
      meta,
      el("h3", "", title(item)),
      el("p", "body-snippet", bodyText(item)),
      el("div", "ref", exact(item.reference)),
    );
    row.addEventListener("click", () => {
      list.querySelectorAll(".selected").forEach((n) => n.classList.remove("selected"));
      row.classList.add("selected");
      showDetail(item).catch(error);
    });
    list.append(row);
  }
}
async function showDetail(item) {
  const panel = $("detail");
  panel.replaceChildren(
    chip(kindNames[item.reference.kind] || item.reference.kind),
    el("h2", "", title(item)),
    el("div", "body-text", bodyText(item)),
  );
  const times = el("dl");
  for (const [key, value] of Object.entries(item.times || {})) {
    times.append(
      el(
        "dt",
        "",
        {
          event: "事件时间",
          observation: "接触时间",
          formation: "形成时间",
          validity: "有效范围",
          recorded: "记录时间",
        }[key] || key,
      ),
      el("dd", "", value ? value.original || JSON.stringify(value) : "未指定"),
    );
  }
  panel.append(times);
  const detail = el("details");
  detail.append(el("summary", "", "完整认知记录"), jsonView(item));
  panel.append(detail);
  if (item.reference.kind === "material")
    panel.append(
      button("下载原件", () =>
        download(
          subjectPath(`/materials/${encodeURIComponent(item.reference.id)}/content`),
          item.filename,
        ),
      ),
    );
  if (item.reference.kind === "source_region")
    panel.append(
      button("下载精确区域", () =>
        download(
          subjectPath(`/regions/${encodeURIComponent(item.reference.id)}/content`),
          item.filename || "region",
        ),
      ),
    );
  panel.append(el("h3", "", "来源"));
  for (const sourceId of item.source_roots || [])
    panel.append(
      button(sourceId, async () => {
        const sources = await api(subjectPath("/sources"));
        const source = sources.find((v) => v.id === sourceId);
        if (!source) throw new Error("来源已不在当前可读范围");
        const value = await api(subjectPath(`/materials/${source.material_id}`));
        panel.append(jsonView(value));
      }),
    );
}
export async function loadSources() {
  const sources = await api(subjectPath("/sources"));
  const rows = sources.map((s) => {
    const check = el("input");
    check.type = "checkbox";
    check.value = s.id;
    check.name = "source-id";
    return [
      check,
      s.source_type || s.kind,
      s.material_id,
      s.id,
      chip(s.active ? "有效" : "已撤回", !s.active),
      s.origin?.conversation_ref || "—",
    ];
  });
  $("source-list").replaceChildren(
    rows.length
      ? table(["选择", "类型", "原件", "接触 / 导入", "状态", "会话来源"], rows)
      : el("div", "empty", "暂无来源"),
  );
}
function selectedSources() {
  const ids = [...document.querySelectorAll("input[name=source-id]:checked")].map((v) => v.value);
  if (!ids.length) throw new Error("请先选择来源");
  return ids;
}
async function previewSources() {
  state.preview = await api(subjectPath("/sources/preview"), {
    method: "POST",
    body: { observation_ids: selectedSources() },
  });
  const p = state.preview;
  $("source-plan").replaceChildren(
    el(
      "p",
      "",
      `${p.retiring.length} 条认知将失去最后支持；${p.retained.length} 条仍有其他来源支持。`,
    ),
    jsonView(p),
  );
}
async function sourceAction(event) {
  const action = event.submitter.value;
  if (
    action === "purge" &&
    !window.confirm("永久清除所选来源的正文及仅由其支持的内容？这项操作无法恢复。")
  )
    return;
  await api(subjectPath(`/sources/${action}`), {
    method: "POST",
    body: {
      operation_id: op(),
      observation_ids: selectedSources(),
      reason: $("source-reason").value,
      expected_preview: action === "trash" ? state.preview?.preview_token : null,
    },
  });
  state.preview = null;
  notice("来源已更新");
  await loadSources();
}
async function importFiles() {
  const files = [...$("import-files").files];
  if (!files.length) throw new Error("请选择资料");
  const status = $("import-status");
  status.replaceChildren();
  for (const file of files) {
    const row = el("p", "", `${file.name} · 正在接纳…`);
    status.append(row);
    const text = file.type.startsWith("text/") || /\.(txt|md|json|jsonl|csv)$/i.test(file.name);
    if (text) {
      const value = await api(subjectPath("/imports/files"), {
        method: "POST",
        body: {
          operation_id: op(),
          inputs: [
            {
              text: await file.text(),
              filename: file.name,
              media_type: file.type || "text/plain",
              file_identity: `browser:${file.name}`,
              source_type: "document",
            },
          ],
          llm_enabled: $("llm").value === "true",
          vectors: $("vectors").value,
        },
      });
      row.textContent = `${file.name} · 已保存 ${value.outputs?.length || 0} 个单位输出${value.skipped_success_ids?.length ? "（复用已完成资料）" : ""}`;
    } else {
      const params = new URLSearchParams({
        namespace_id: state.namespace,
        owner_subject_id: state.subject,
        source: file.name,
        operation_id: op(),
      });
      const upload = await api(`/api/media/materials?${params}`, {
        method: "POST",
        headers: { "Content-Type": file.type || "application/octet-stream" },
        body: file,
      });
      const result = await api(subjectPath("/media/adopt"), {
        method: "POST",
        body: {
          operation_id: op(),
          material_id: upload.result.id,
          strategy: $("media-strategy").value,
          instruction:
            $("media-instruction").value || "理解并记住原媒体中的内容，保持材料语言和准确来源。",
        },
      });
      row.textContent = `${file.name} · 已形成 ${result.cognition.length} 条认知`;
    }
  }
  notice("所选资料已接纳");
}
function parseRef(value) {
  const match = value.trim().match(/^([^:]+):(.+)@(\d+)$/);
  if (!match) throw new Error(`引用格式应为 kind:id@revision：${value}`);
  return { kind: match[1], id: match[2], revision: Number(match[3]) };
}
async function admitContext() {
  const references = $("context-refs")
    .value.split("\n")
    .filter((v) => v.trim())
    .map(parseRef);
  const value = await api(subjectPath("/contexts"), {
    method: "POST",
    body: {
      operation_id: op(),
      references,
      purpose: $("context-purpose").value,
      kind: $("context-kind").value,
    },
  });
  $("context-result").replaceChildren(jsonView(value));
  notice("工作上下文已接纳");
}
export function initializeKnowledge() {
  form("query-form", query);
  $("refresh-query").onclick = () => busy($("refresh-query"), query);
  form("import-form", importFiles);
  $("refresh-sources").onclick = () => busy($("refresh-sources"), loadSources);
  $("source-preview").onclick = () => busy($("source-preview"), previewSources);
  form("source-action", sourceAction);
  form("context-form", admitContext);
}
