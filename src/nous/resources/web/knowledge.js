import { t, number, date, message } from "./i18n.js";
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
  notify,
  error,
  busy,
  jsonView,
  chip,
  download,
} from "./client.js";
const kinds = new Set([
  "memory",
  "memory_semantics",
  "concept",
  "entity",
  "episode",
  "journal",
  "schema",
  "association",
  "self_facet",
  "narrative_thread",
  "domain_knowledge",
  "social_relation",
  "language_convention",
  "desire",
  "concern",
  "material",
  "source_region",
]);
const kindName = (kind) => (kinds.has(kind) ? t(`web-kind-${kind.replaceAll("_", "-")}`) : kind);
let lastQuery, selectedItem, lastSources;
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
  lastQuery = result;
  selectedItem = undefined;
  renderQuery(result);
}
function renderQuery(result) {
  message($("result-count"), "web-result-count", { count: result.hits.length });
  $("coverage").textContent = result.coverage.complete
    ? t("web-coverage-complete")
    : t("web-coverage-incomplete");
  const list = $("results");
  list.replaceChildren();
  if (!result.hits.length) {
    const empty = el("div", "empty");
    empty.append(el("h2", "", t("web-no-matches")), el("p", "", t("web-no-matches-help")));
    list.append(empty);
    return;
  }
  for (const hit of result.hits) {
    const item = hit.object;
    const row = el("button", "result-row");
    const meta = el("div", "row-meta");
    meta.append(
      el("span", "", kindName(item.reference.kind)),
      el("span", "", number(hit.score, { minimumFractionDigits: 3, maximumFractionDigits: 3 })),
    );
    row.append(
      meta,
      el("h3", "", title(item)),
      el("p", "body-snippet", bodyText(item)),
      el("div", "ref", item.addresses?.exact || exact(item.reference)),
    );
    row.addEventListener("click", () => {
      list.querySelectorAll(".selected").forEach((n) => n.classList.remove("selected"));
      row.classList.add("selected");
      selectedItem = item;
      showDetail(item).catch(error);
    });
    list.append(row);
  }
}
function timeLabel(value) {
  if (!value) return t("web-unspecified");
  if (value.original) return value.original;
  if (value.coordinate !== undefined) {
    if (value.domain === "unix_ms") return date(value.coordinate) + " UTC";
    return `${value.coordinate} · ${value.domain}`;
  }
  if (value.start !== undefined || value.end !== undefined)
    return [value.start, value.end]
      .map((point) =>
        point == null ? t("web-unbounded") : timeLabel({ domain: value.domain, coordinate: point }),
      )
      .join(" — ");
  return t("web-unspecified");
}

async function showDetail(item) {
  const panel = $("detail");
  panel.replaceChildren(
    chip(kindName(item.reference.kind)),
    el("h2", "", title(item)),
    el("div", "ref", item.addresses?.exact || exact(item.reference)),
    el("div", "body-text", bodyText(item)),
  );
  const times = el("dl");
  for (const [key, value] of Object.entries(item.times || {})) {
    times.append(
      el(
        "dt",
        "",
        {
          event: t("web-time-event"),
          observation: t("web-time-observation"),
          formation: t("web-time-formation"),
          validity: t("web-time-validity"),
          recorded: t("web-time-recorded"),
        }[key] || key,
      ),
      el("dd", "", timeLabel(value)),
    );
  }
  panel.append(times);
  const detail = el("details");
  detail.append(el("summary", "", t("web-full-record")), jsonView(item));
  panel.append(detail);
  if (item.reference.kind === "material")
    panel.append(
      button(t("web-download-original"), () =>
        download(
          subjectPath(`/materials/${encodeURIComponent(item.reference.id)}/content`),
          item.filename,
        ),
      ),
    );
  if (item.reference.kind === "source_region")
    panel.append(
      button(t("web-download-region"), () =>
        download(
          subjectPath(`/regions/${encodeURIComponent(item.reference.id)}/content`),
          item.filename || "region",
        ),
      ),
    );
  panel.append(el("h3", "", t("web-source")));
  for (const sourceId of item.source_roots || [])
    panel.append(
      button(sourceId, async () => {
        const sources = await api(subjectPath("/sources"));
        const source = sources.find((v) => v.id === sourceId);
        if (!source) throw new Error(t("web-source-unreadable"));
        const value = await api(subjectPath(`/materials/${source.material_id}`));
        panel.append(jsonView(value));
      }),
    );
}
export async function loadSources() {
  lastSources = await api(subjectPath("/sources"));
  renderSources(lastSources);
}
function renderSources(sources) {
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
      chip(s.active ? t("web-source-active") : t("web-source-withdrawn"), !s.active),
      s.origin?.conversation_ref || "—",
    ];
  });
  $("source-list").replaceChildren(
    rows.length
      ? table(
          [
            t("web-select"),
            t("web-type"),
            t("web-kind-material"),
            t("web-observation-import"),
            t("web-status"),
            t("web-conversation-origin"),
          ],
          rows,
        )
      : el("div", "empty", t("web-no-sources")),
  );
}
function selectedSources() {
  const ids = [...document.querySelectorAll("input[name=source-id]:checked")].map((v) => v.value);
  if (!ids.length) throw new Error(t("web-select-sources"));
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
      t("web-source-impact", { retiring: p.retiring.length, retained: p.retained.length }),
    ),
    jsonView(p),
  );
}
async function sourceAction(event) {
  const action = event.submitter.value;
  if (action === "purge" && !window.confirm(t("web-purge-confirm"))) return;
  await api(subjectPath(`/sources/${action}`), {
    method: "POST",
    body: {
      operation_id: op(),
      observation_ids: selectedSources(),
      reason: $("source-reason").value,
      expected_preview:
        action === "trash" || action === "purge" ? state.preview?.preview_token : null,
    },
  });
  state.preview = null;
  notify("web-sources-updated");
  await loadSources();
}
async function importFiles() {
  const files = [...$("import-files").files];
  if (!files.length) throw new Error(t("web-select-files"));
  const status = $("import-status");
  status.replaceChildren();
  for (const file of files) {
    const row = message(el("p"), "web-importing-file", { filename: file.name });
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
              format: file.name.toLowerCase().endsWith(".jsonl")
                ? "jsonl"
                : file.name.toLowerCase().endsWith(".json")
                  ? "json"
                  : "text",
              file_identity: `browser:${file.name}`,
              source_type: "document",
            },
          ],
          llm_enabled: $("llm").value === "true",
          vectors: $("vectors").value,
        },
      });
      message(row, value.skipped_success_ids?.length ? "web-reused-file" : "web-imported-file", {
        filename: file.name,
        count: value.outputs?.length || 0,
      });
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
          instruction: $("media-instruction").value || t("web-default-media-instruction"),
        },
      });
      message(row, "web-media-formed", { filename: file.name, count: result.cognition.length });
    }
  }
  notify("web-files-adopted");
}
async function parseRef(value) {
  const match = value.trim().match(/^([^:]+):(.+)@(\d+)$/);
  if (match) return { kind: match[1], id: match[2], revision: Number(match[3]) };
  const resolved = await api("/api/references", {
    method: "POST",
    body: { subject_id: state.subject, reference: value.trim() },
  });
  if (resolved.reference?.kind !== "cognition")
    throw new Error(t("web-invalid-reference", { reference: value }));
  return resolved.reference.reference;
}
async function admitContext() {
  const references = await Promise.all(
    $("context-refs")
      .value.split("\n")
      .filter((v) => v.trim())
      .map(parseRef),
  );
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
  notify("web-context-admitted");
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

export function resetKnowledge() {
  lastQuery = selectedItem = lastSources = undefined;
  state.preview = null;
  for (const id of [
    "results",
    "detail",
    "source-list",
    "source-plan",
    "context-result",
    "import-status",
  ])
    $(id).replaceChildren();
  message($("result-count"), "web-not-queried");
  $("coverage").textContent = "";
  message($("detail"), "web-detail-empty");
}
export function renderKnowledge() {
  if (lastQuery) renderQuery(lastQuery);
  if (selectedItem) showDetail(selectedItem).catch(error);
  if (lastSources) renderSources(lastSources);
}
